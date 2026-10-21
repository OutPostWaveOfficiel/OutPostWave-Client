//! Safety numbers: out-of-band verification between two contacts.

use crate::PublicKey;

/// Domain-separation context.
const SAFETY_CONTEXT: &str = "OutpostWave 2026-10 safety number v1";

/// Returns the 60-digit safety number of two identities, as 12 groups of 5 digits.
///
/// Both sides compute the same value regardless of argument order. Each half is derived from one
/// key (about 100 bits each), so an attacker cannot forge a matching key.
#[must_use]
pub fn safety_number(a: &PublicKey, b: &PublicKey) -> String {
    let (first, second) = if a.to_bytes() <= b.to_bytes() {
        (a, b)
    } else {
        (b, a)
    };
    let mut groups = Vec::with_capacity(12);
    for key in [first, second] {
        let mut hasher = blake3::Hasher::new_derive_key(SAFETY_CONTEXT);
        hasher.update(&key.to_bytes());
        let digest = hasher.finalize();
        for chunk in digest.as_bytes()[..30].chunks_exact(5) {
            let mut buf = [0u8; 8];
            buf[3..].copy_from_slice(chunk);
            groups.push(format!("{:05}", u64::from_be_bytes(buf) % 100_000));
        }
    }
    groups.join(" ")
}

#[cfg(test)]
mod tests {
    use super::safety_number;
    use crate::{MasterKey, RecoveryPhrase};

    #[test]
    fn symmetric_and_well_formed() {
        let a = MasterKey::from_phrase(&RecoveryPhrase::generate().unwrap()).public_key();
        let b = MasterKey::from_phrase(&RecoveryPhrase::generate().unwrap()).public_key();
        let n = safety_number(&a, &b);
        assert_eq!(n, safety_number(&b, &a));
        assert_eq!(n.len(), 12 * 5 + 11);
        assert!(
            n.split(' ')
                .all(|g| g.len() == 5 && g.bytes().all(|c| c.is_ascii_digit()))
        );
        assert_ne!(n, safety_number(&a, &a));
    }
}
