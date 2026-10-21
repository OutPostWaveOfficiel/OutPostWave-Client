//! Chooses which chunk to ask from which source.
//!
//! - Pull-based: each source gets a window of parallel requests; faster sources complete sooner
//!   and get more work, and their window grows with their measured throughput.
//! - Rarest first: among the next eligible chunks (a bounded look-ahead from a cursor, to keep
//!   disk access near-sequential and the cost bounded on huge files), the one held by the fewest
//!   sources is chosen.
//! - A source that fails repeatedly or sends bad data is dropped.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::Duration;

use crate::Bitmap;

/// Eligible chunks examined per pick.
const LOOK_AHEAD: usize = 256;
/// Parallel requests per source.
const MIN_WINDOW: usize = 2;
const START_WINDOW: usize = 4;
const MAX_WINDOW: usize = 32;
/// Data in flight per source, in seconds of its measured throughput.
const TARGET_SECONDS: f64 = 0.5;
/// Consecutive failures before a source is dropped.
const MAX_FAILURES: u32 = 4;

#[derive(Debug)]
struct SourceState {
    have: Bitmap,
    inflight: usize,
    /// Throughput estimate, bytes per second (EWMA).
    rate: f64,
    failures: u32,
}

/// Download plan of one file.
#[derive(Debug)]
pub(crate) struct Scheduler<S> {
    done: Bitmap,
    inflight: Bitmap,
    availability: Vec<u16>,
    sources: HashMap<S, SourceState>,
    cursor: u32,
    chunk_size: usize,
}

impl<S: Clone + Eq + Hash> Scheduler<S> {
    pub(crate) fn new(done: Bitmap, chunk_size: u32) -> Self {
        let n = done.len();
        Self {
            inflight: Bitmap::new(n),
            availability: vec![0; n as usize],
            done,
            sources: HashMap::new(),
            cursor: 0,
            chunk_size: chunk_size as usize,
        }
    }

    pub(crate) const fn done(&self) -> &Bitmap {
        &self.done
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.done.is_full()
    }

    pub(crate) fn sources(&self) -> Vec<S> {
        self.sources.keys().cloned().collect()
    }

    /// Adds a source or updates what it holds.
    pub(crate) fn set_have(&mut self, s: S, have: Bitmap) {
        if have.len() != self.done.len() {
            return;
        }
        if let Some(old) = self.sources.get(&s) {
            for i in 0..have.len() {
                if old.have.get(i) {
                    self.availability[i as usize] -= 1;
                }
            }
        }
        for i in 0..have.len() {
            if have.get(i) {
                self.availability[i as usize] += 1;
            }
        }
        let st = self.sources.entry(s).or_insert(SourceState {
            have: Bitmap::new(0),
            inflight: 0,
            rate: 0.0,
            failures: 0,
        });
        st.have = have;
    }

    pub(crate) fn remove(&mut self, s: &S) {
        if let Some(old) = self.sources.remove(s) {
            for i in 0..old.have.len() {
                if old.have.get(i) {
                    self.availability[i as usize] -= 1;
                }
            }
        }
    }

    fn window(st: &SourceState, chunk_size: usize) -> usize {
        if st.rate <= 0.0 {
            return START_WINDOW;
        }
        // Truncation is fine: this is a coarse target.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let w = (st.rate * TARGET_SECONDS / chunk_size as f64).ceil() as usize + 1;
        w.clamp(MIN_WINDOW, MAX_WINDOW)
    }

    /// Next chunk to request from `s`, if its window has room and it holds a needed chunk.
    pub(crate) fn next_for(&mut self, s: &S) -> Option<u32> {
        let chunk_size = self.chunk_size;
        let st = self.sources.get(s)?;
        if st.inflight >= Self::window(st, chunk_size) {
            return None;
        }
        let n = self.done.len();
        let mut best: Option<(u16, u32)> = None;
        let mut seen = 0;
        for k in 0..n {
            let i = (self.cursor + k) % n;
            if self.done.get(i) || self.inflight.get(i) || !st.have.get(i) {
                continue;
            }
            let a = self.availability[i as usize];
            if best.is_none_or(|(b, _)| a < b) {
                best = Some((a, i));
                if a <= 1 {
                    break;
                }
            }
            seen += 1;
            if seen >= LOOK_AHEAD {
                break;
            }
        }
        let (_, i) = best?;
        self.inflight.set(i);
        if let Some(st) = self.sources.get_mut(s) {
            st.inflight += 1;
        }
        if i == self.cursor {
            self.cursor = (i + 1) % n.max(1);
        }
        Some(i)
    }

    fn release(&mut self, s: &S, i: u32) {
        self.inflight.clear(i);
        if let Some(st) = self.sources.get_mut(s) {
            st.inflight = st.inflight.saturating_sub(1);
        }
    }

    /// Chunk `i` arrived from `s` (verified) after `elapsed`.
    pub(crate) fn complete(&mut self, s: &S, i: u32, bytes: usize, elapsed: Duration) {
        self.release(s, i);
        self.done.set(i);
        if let Some(st) = self.sources.get_mut(s) {
            st.failures = 0;
            #[allow(clippy::cast_precision_loss)]
            let sample = bytes as f64 / elapsed.as_secs_f64().max(1e-3);
            // Several requests run in parallel: the source rate is the sum of their rates.
            #[allow(clippy::cast_precision_loss)]
            let sample = sample * (st.inflight + 1) as f64;
            st.rate = if st.rate == 0.0 {
                sample
            } else {
                st.rate * 0.8 + sample * 0.2
            };
        }
    }

    /// Request of chunk `i` to `s` failed. Returns `true` if the source was dropped.
    pub(crate) fn failed(&mut self, s: &S, i: u32, drop_now: bool) -> bool {
        self.release(s, i);
        let drop = match self.sources.get_mut(s) {
            Some(st) => {
                st.failures += 1;
                drop_now || st.failures >= MAX_FAILURES
            }
            None => false,
        };
        if drop {
            self.remove(s);
        }
        drop
    }

    /// The source does not have chunk `i` after all.
    pub(crate) fn missing(&mut self, s: &S, i: u32) {
        self.release(s, i);
        if let Some(st) = self.sources.get_mut(s) {
            if st.have.clear(i) {
                self.availability[i as usize] = self.availability[i as usize].saturating_sub(1);
            }
        }
    }

    /// Some needed chunk is not held by any current source.
    pub(crate) fn lacks_sources(&self) -> bool {
        (0..self.done.len()).any(|i| !self.done.get(i) && self.availability[i as usize] == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rarest_first_and_windows() {
        let mut s: Scheduler<&str> = Scheduler::new(Bitmap::new(8), 1024);
        let mut a = Bitmap::new(8);
        let mut b = Bitmap::new(8);
        for i in 0..8 {
            a.set(i);
        }
        b.set(5);
        s.set_have("a", a);
        s.set_have("b", b);
        // Chunk 5 is held by two sources: "a" picks the rare ones first.
        let picks: Vec<u32> = (0..4).filter_map(|_| s.next_for(&"a")).collect();
        assert_eq!(picks, vec![0, 1, 2, 3]);
        assert_eq!(s.next_for(&"a"), None, "window full");
        assert_eq!(s.next_for(&"b"), Some(5));
        s.complete(&"a", 0, 1024, Duration::from_millis(10));
        assert_eq!(s.next_for(&"a"), Some(4));
        assert!(s.failed(&"b", 5, true), "bad data drops the source");
        assert!(!s.sources().contains(&"b"));
        assert_eq!(s.next_for(&"a"), Some(5));
        s.missing(&"a", 1);
        assert!(s.lacks_sources());
    }
}
