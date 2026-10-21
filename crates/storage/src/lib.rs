//! Encrypted local SQLite storage.
//!
//! Status: skeleton (PoC roadmap step 0). The implementation will land in later steps.

/// Name of this crate, exposed for diagnostics and logs.
pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_set() {
        assert!(super::CRATE_NAME.starts_with("ow-"));
    }
}
