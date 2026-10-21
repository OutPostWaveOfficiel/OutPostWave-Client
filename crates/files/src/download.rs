//! Swarm download driver.

use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use tokio::sync::{mpsc, watch};

use crate::scheduler::Scheduler;
use crate::{
    Bitmap, FileId, FileKey, HASHES_PER_PAGE, Manifest, PartFile, Request, Response, Status,
};

/// Largest accepted manifest (enough for files of several TiB).
const MAX_MANIFEST: usize = 256 * 1024 * 1024;
/// Time allowed for one request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Progress events at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// The bitmap is persisted (after an fsync) at most this often.
const PERSIST_EVERY: Duration = Duration::from_secs(1);
/// How often sources are asked again what they hold, while some chunk has no source.
const REFRESH_EVERY: Duration = Duration::from_secs(3);
/// Without anything to request for this long, the download pauses.
const STALL_AFTER: Duration = Duration::from_secs(10);

/// Network access used by downloads.
pub trait Transport: Send + Sync {
    /// Identifies a source (a peer).
    type Source: Clone + Eq + Hash + Debug + Send + Sync + 'static;

    /// Sends an encoded [`Request`] to `source` and returns the encoded [`Response`].
    fn request(
        &self,
        source: &Self::Source,
        request: Vec<u8>,
    ) -> impl Future<Output = Result<Vec<u8>, String>> + Send;
}

async fn ask<T: Transport>(t: &T, s: &T::Source, r: Request) -> Result<Response, String> {
    // Small requests must answer fast; chunks may take longer on slow links.
    let limit = if matches!(r, Request::Chunk(..)) {
        REQUEST_TIMEOUT
    } else {
        Duration::from_secs(10)
    };
    let bytes = tokio::time::timeout(limit, t.request(s, r.encode()))
        .await
        .map_err(|_| "timeout".to_owned())??;
    Response::decode(bytes).ok_or_else(|| "malformed response".to_owned())
}

/// Downloads and verifies the manifest of `id` from the first source that has it.
///
/// # Errors
///
/// A description of the last failure if no source could provide a valid manifest.
pub async fn fetch_manifest<T: Transport>(
    t: &T,
    sources: &[T::Source],
    id: &FileId,
) -> Result<Manifest, String> {
    let mut last = "no source".to_owned();
    'sources: for s in sources {
        let mut bytes = Vec::new();
        let mut total = None;
        for page in 0u32.. {
            let r = match ask(t, s, Request::Manifest(*id, page)).await {
                Ok(r) if r.status == Status::Ok && r.body.len() >= 4 => r,
                Ok(r) => {
                    last = format!("{:?}", r.status);
                    continue 'sources;
                }
                Err(e) => {
                    last = e;
                    continue 'sources;
                }
            };
            let len = u32::from_be_bytes([r.body[0], r.body[1], r.body[2], r.body[3]]) as usize;
            if len > MAX_MANIFEST || total.is_some_and(|t| t != len) {
                last = "manifest too large".to_owned();
                continue 'sources;
            }
            total = Some(len);
            bytes.extend_from_slice(&r.body[4..]);
            if bytes.len() >= len || r.body.len() < 4 + HASHES_PER_PAGE * 32 {
                break;
            }
        }
        match Manifest::parse(&bytes, id) {
            Ok(m) => return Ok(m),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// Progress of a download.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    /// Chunks received.
    pub chunks: u32,
    /// Total chunks.
    pub total: u32,
    /// Bytes received (plaintext).
    pub bytes: u64,
    /// File size.
    pub size: u64,
    /// Current throughput, bytes per second.
    pub rate: f64,
    /// Sources currently usable.
    pub sources: usize,
}

/// How a download ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every chunk was received and verified.
    Complete,
    /// No source can provide the missing chunks right now: retry when a source comes back.
    Stalled(Bitmap),
    /// Cancelled (paused) by the caller.
    Cancelled(Bitmap),
    /// The original file changed at the sender: it cannot be completed.
    Changed(Bitmap),
    /// Local disk error.
    Failed(Bitmap, String),
}

/// A download to run.
pub struct Download<'a, T: Transport> {
    /// Verified manifest.
    pub manifest: &'a Manifest,
    /// File key.
    pub key: &'a FileKey,
    /// Destination.
    pub part: &'a PartFile,
    /// Chunks already received (resume).
    pub done: Bitmap,
    /// Initial sources.
    pub sources: Vec<T::Source>,
    /// More sources discovered during the download.
    pub new_sources: Option<mpsc::UnboundedReceiver<T::Source>>,
    /// Set to `true` to stop.
    pub cancel: watch::Receiver<bool>,
}

type Fetch<'f, S> =
    Pin<Box<dyn Future<Output = (S, u32, Instant, Result<Response, String>)> + Send + 'f>>;

/// Runs a swarm download. `on_progress` is called at most every 250 ms; when its bitmap argument
/// is `Some`, the data is on disk (fsync) and the bitmap should be persisted for resume.
pub async fn download<T: Transport>(
    t: &T,
    mut d: Download<'_, T>,
    mut on_progress: impl FnMut(Progress, Option<&Bitmap>) + Send,
) -> Outcome {
    let m = d.manifest;
    if d.done.len() != m.count() {
        d.done = Bitmap::new(m.count());
    }
    let mut sched: Scheduler<T::Source> = Scheduler::new(d.done.clone(), m.chunk_size());
    let mut known: Vec<T::Source> = Vec::new();
    for s in d.sources.drain(..) {
        if !known.contains(&s) {
            known.push(s);
        }
    }
    let bytes_of = |b: &Bitmap| -> u64 {
        (0..b.len())
            .filter(|&i| b.get(i))
            .map(|i| m.chunk_len(i) as u64)
            .sum()
    };
    let mut bytes = bytes_of(sched.done());
    let mut inflight: FuturesUnordered<Fetch<'_, T::Source>> = FuturesUnordered::new();
    let mut last_progress = Instant::now() - PROGRESS_EVERY;
    let mut last_persist = Instant::now();
    let mut last_refresh: Option<Instant> = None;
    let mut idle_since: Option<Instant> = None;
    let mut window_bytes = 0u64;
    let mut window_start = Instant::now();
    let mut rate = 0.0f64;
    let mut dirty = false;

    let finish = |part: &PartFile,
                  done: &Bitmap,
                  on_progress: &mut dyn FnMut(Progress, Option<&Bitmap>),
                  p: Progress|
     -> Result<(), String> {
        part.sync().map_err(|e| e.to_string())?;
        on_progress(p, Some(done));
        Ok(())
    };

    loop {
        let progress = Progress {
            chunks: sched.done().count(),
            total: m.count(),
            bytes,
            size: m.size(),
            rate,
            sources: sched.sources().len(),
        };
        if sched.is_complete() {
            if let Err(e) = finish(d.part, sched.done(), &mut on_progress, progress) {
                return Outcome::Failed(sched.done().clone(), e);
            }
            return Outcome::Complete;
        }
        if *d.cancel.borrow() {
            let _ = finish(d.part, sched.done(), &mut on_progress, progress);
            return Outcome::Cancelled(sched.done().clone());
        }

        // Ask (again) what sources hold: at start, and while some chunk has no source.
        let refresh_due = last_refresh.is_none_or(|l| l.elapsed() >= REFRESH_EVERY);
        if refresh_due && (last_refresh.is_none() || sched.lacks_sources()) {
            last_refresh = Some(Instant::now());
            for s in &known {
                if let Ok(r) = ask(t, s, Request::Have(*m.id())).await {
                    match r.status {
                        Status::Ok => {
                            if let Some(have) = Bitmap::from_bytes(&r.body, m.count()) {
                                sched.set_have(s.clone(), have);
                            }
                        }
                        Status::Changed => return Outcome::Changed(sched.done().clone()),
                        _ => sched.remove(s),
                    }
                } else {
                    sched.remove(s);
                }
            }
        }

        // Fill every source's window.
        for s in sched.sources() {
            while let Some(i) = sched.next_for(&s) {
                let s2 = s.clone();
                let id = *m.id();
                inflight.push(Box::pin(async move {
                    let start = Instant::now();
                    let r = ask(t, &s2, Request::Chunk(id, i)).await;
                    (s2, i, start, r)
                }));
            }
        }

        if inflight.is_empty() {
            let since = *idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= STALL_AFTER {
                let _ = finish(d.part, sched.done(), &mut on_progress, progress);
                return Outcome::Stalled(sched.done().clone());
            }
        } else {
            idle_since = None;
        }

        let tick = tokio::time::sleep(Duration::from_millis(500));
        tokio::select! {
            Some((s, i, start, r)) = inflight.next(), if !inflight.is_empty() => {
                match r {
                    Ok(Response { status: Status::Ok, body }) => {
                        let valid = m.hash(i).is_some_and(|h| blake3::hash(&body).as_bytes() == h);
                        let plain = if valid { d.key.open(i, &body) } else { None };
                        match plain {
                            Some(plain) if plain.len() == m.chunk_len(i) => {
                                if let Err(e) = d.part.write(m.offset(i), &plain) {
                                    return Outcome::Failed(sched.done().clone(), e.to_string());
                                }
                                sched.complete(&s, i, body.len(), start.elapsed());
                                bytes += plain.len() as u64;
                                window_bytes += plain.len() as u64;
                                dirty = true;
                            }
                            _ => {
                                // Forged or corrupted data: never trust this source again.
                                tracing::warn!(source = ?s, chunk = i, "invalid chunk, source dropped");
                                sched.failed(&s, i, true);
                                known.retain(|k| k != &s);
                            }
                        }
                    }
                    Ok(Response { status: Status::Missing, .. }) => sched.missing(&s, i),
                    Ok(Response { status: Status::Busy, .. }) => {
                        sched.failed(&s, i, false);
                    }
                    Ok(Response { status: Status::Changed, .. }) => {
                        let _ = finish(d.part, sched.done(), &mut on_progress, progress);
                        return Outcome::Changed(sched.done().clone());
                    }
                    Ok(Response { status: Status::Unknown, .. }) | Err(_) => {
                        sched.failed(&s, i, false);
                    }
                }
            }
            Some(s) = async {
                match d.new_sources.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if !known.contains(&s) {
                    known.push(s);
                }
                last_refresh = None;
            }
            _ = d.cancel.changed() => {}
            () = tick => {}
        }

        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            #[allow(clippy::cast_precision_loss)]
            let sample = window_bytes as f64 / elapsed.as_secs_f64();
            rate = if rate == 0.0 {
                sample
            } else {
                rate * 0.5 + sample * 0.5
            };
            window_bytes = 0;
            window_start = Instant::now();
        }
        if last_progress.elapsed() >= PROGRESS_EVERY {
            last_progress = Instant::now();
            let p = Progress {
                chunks: sched.done().count(),
                total: m.count(),
                bytes,
                size: m.size(),
                rate,
                sources: sched.sources().len(),
            };
            if dirty && last_persist.elapsed() >= PERSIST_EVERY {
                if let Err(e) = d.part.sync() {
                    return Outcome::Failed(sched.done().clone(), e.to_string());
                }
                last_persist = Instant::now();
                dirty = false;
                on_progress(p, Some(sched.done()));
            } else {
                on_progress(p, None);
            }
        }
    }
}
