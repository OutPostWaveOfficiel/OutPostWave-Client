//! User handles (unique usernames).

use core::fmt;
use core::str::FromStr;

use crate::IdentityError;

/// Minimum name length.
pub const MIN_NAME_LEN: usize = 3;
/// Maximum name length.
pub const MAX_NAME_LEN: usize = 32;

/// A username, unique across OutpostWave (enforced by the account registry).
///
/// Rules: `a-z`, `0-9`, `_`, `.`; 3 to 32 characters; no leading/trailing or doubled `.`.
/// ASCII-only to prevent homoglyph impersonation. Input is lowercased.
///
/// A name says *who claims to be* someone; the binding to keys is checked with
/// [`safety_number`](crate::safety_number).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle(String);

impl Handle {
    /// Validates and normalizes a name.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidName`] if the name breaks the rules.
    pub fn new(name: &str) -> Result<Self, IdentityError> {
        let name = name.trim().to_ascii_lowercase();
        let ok = (MIN_NAME_LEN..=MAX_NAME_LEN).contains(&name.len())
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
            && !name.starts_with('.')
            && !name.ends_with('.')
            && !name.contains("..");
        if ok {
            Ok(Self(name))
        } else {
            Err(IdentityError::InvalidName)
        }
    }

    /// The normalized name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Handle {
    type Err = IdentityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::Handle;
    use crate::IdentityError;

    #[test]
    fn normalizes() {
        assert_eq!(Handle::new("  Jean_Pol ").unwrap().to_string(), "jean_pol");
        assert_eq!("ABC".parse::<Handle>().unwrap().name(), "abc");
    }

    #[test]
    fn name_rules() {
        for bad in [
            "ab",
            "a b",
            ".abc",
            "abc.",
            "a..b",
            "jéan",
            "ab-c",
            "a#b1",
            &"a".repeat(33),
        ] {
            assert_eq!(Handle::new(bad), Err(IdentityError::InvalidName), "{bad}");
        }
        for good in ["abc", "a.b.c", "x_1", &"a".repeat(32)] {
            assert!(Handle::new(good).is_ok(), "{good}");
        }
    }
}
