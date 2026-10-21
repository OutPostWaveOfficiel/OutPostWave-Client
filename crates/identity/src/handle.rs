//! Human-readable handles: `name#tag`.

use core::fmt;
use core::str::FromStr;

use crate::{IdentityError, PublicKey};

/// Minimum name length.
pub const MIN_NAME_LEN: usize = 3;
/// Maximum name length.
pub const MAX_NAME_LEN: usize = 32;
/// Tag length, in Crockford base32 characters (20 bits).
pub const TAG_LEN: usize = 4;

/// A handle `name#tag`.
///
/// - `name` is unique (enforced by the account registry). Allowed: `a-z`, `0-9`, `_`, `.`;
///   3 to 32 characters; no leading/trailing or doubled `.`. ASCII-only to prevent homoglyph
///   impersonation. Input is lowercased.
/// - `tag` is the first 4 characters of the master key fingerprint. It is a quick visual
///   hint only (20 bits can be brute-forced): real verification uses
///   [`safety_number`](crate::safety_number).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Handle {
    name: String,
    tag: String,
}

impl Handle {
    /// Builds the handle of `master` with the given name.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidName`] if the name breaks the rules.
    pub fn new(name: &str, master: &PublicKey) -> Result<Self, IdentityError> {
        Ok(Self {
            name: normalize_name(name)?,
            tag: tag_of(master),
        })
    }

    /// Name part.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Tag part.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Checks that the tag matches `master`.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::TagMismatch`] if it does not.
    pub fn verify(&self, master: &PublicKey) -> Result<(), IdentityError> {
        if self.tag == tag_of(master) {
            Ok(())
        } else {
            Err(IdentityError::TagMismatch)
        }
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.name, self.tag)
    }
}

impl FromStr for Handle {
    type Err = IdentityError;

    /// Parses `name#tag` (syntax only; use [`Handle::verify`] against the key).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, tag) = s.split_once('#').ok_or(IdentityError::InvalidHandle)?;
        let tag = tag.to_ascii_lowercase();
        let valid_tag = tag.len() == TAG_LEN
            && tag
                .bytes()
                .all(|b| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&b));
        if !valid_tag {
            return Err(IdentityError::InvalidHandle);
        }
        Ok(Self {
            name: normalize_name(name)?,
            tag,
        })
    }
}

fn tag_of(master: &PublicKey) -> String {
    master.fingerprint().to_string()[..TAG_LEN].to_owned()
}

fn normalize_name(name: &str) -> Result<String, IdentityError> {
    let name = name.to_ascii_lowercase();
    let ok = (MIN_NAME_LEN..=MAX_NAME_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..");
    if ok {
        Ok(name)
    } else {
        Err(IdentityError::InvalidName)
    }
}

#[cfg(test)]
mod tests {
    use super::Handle;
    use crate::{IdentityError, MasterKey, RecoveryPhrase};

    fn key() -> crate::PublicKey {
        MasterKey::from_phrase(&RecoveryPhrase::generate().unwrap()).public_key()
    }

    #[test]
    fn build_display_parse_verify() {
        let k = key();
        let h = Handle::new("Jean_Pol", &k).unwrap();
        let s = h.to_string();
        assert!(s.starts_with("jean_pol#"));
        assert_eq!(s.len(), "jean_pol#".len() + 4);
        let parsed: Handle = s.to_uppercase().parse().unwrap();
        assert_eq!(parsed, h);
        parsed.verify(&k).unwrap();
    }

    #[test]
    fn wrong_key_is_detected_most_of_the_time() {
        let h = Handle::new("alice", &key()).unwrap();
        let mismatches = (0..32).filter(|_| h.verify(&key()).is_err()).count();
        assert!(mismatches >= 31);
    }

    #[test]
    fn name_rules() {
        let k = key();
        for bad in [
            "ab",
            "a b",
            ".abc",
            "abc.",
            "a..b",
            "jéan",
            "ab-c",
            &"a".repeat(33),
        ] {
            assert_eq!(
                Handle::new(bad, &k),
                Err(IdentityError::InvalidName),
                "{bad}"
            );
        }
        for good in ["abc", "a.b.c", "x_1", &"a".repeat(32)] {
            assert!(Handle::new(good, &k).is_ok(), "{good}");
        }
    }

    #[test]
    fn parse_errors() {
        for bad in ["alice", "alice#", "alice#abc", "alice#abcde", "alice#iiii"] {
            assert!(bad.parse::<Handle>().is_err(), "{bad}");
        }
    }
}
