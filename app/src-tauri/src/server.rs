//! Account server and relay to use (test phase).
//!
//! Resolution order:
//! 1. `OW_REGISTRY` / `OW_RELAY` environment variables (development);
//! 2. the host saved from the app (`server.json` in the app config folder);
//! 3. the host baked in at build time (`OW_DEFAULT_SERVER=<ip or name>` when running
//!    `tauri build`);
//! 4. debug builds: this machine (`127.0.0.1`).
//!
//! A host `H` means: registry `http://H:8787` and the development relay `H:4001` (QUIC), whose
//! peer id is derived from the public development seed. Production servers will replace this.

use std::path::{Path, PathBuf};

use ow_messaging::RegistryClient;
use ow_p2p::Multiaddr;

/// Registry port of a test server.
const REGISTRY_PORT: u16 = 8787;

/// Resolved server configuration.
#[derive(Clone, Default)]
pub struct Server {
    /// Host chosen in the app or at build time (`None` with environment overrides or nothing).
    pub host: Option<String>,
    /// Account server.
    pub registry: Option<RegistryClient>,
    /// Relays.
    pub relays: Vec<Multiaddr>,
}

fn file(config_dir: &Path) -> PathBuf {
    config_dir.join("server.json")
}

/// Validates a host: IPv4/IPv6 address or DNS name.
#[must_use]
pub fn valid_host(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
        || (!host.is_empty()
            && host.len() <= 253
            && host.split('.').all(|l| {
                !l.is_empty()
                    && l.len() <= 63
                    && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }))
}

fn dev_relay(host: &str) -> Option<Multiaddr> {
    let key = ow_identity::DeviceKey::from_secret_bytes(&ow_p2p::DEV_RELAY_SEED);
    let id = ow_p2p::peer_id_of(&key.public_key().to_bytes()).ok()?;
    let proto = match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(_)) => "ip4",
        Ok(std::net::IpAddr::V6(_)) => "ip6",
        Err(_) => "dns",
    };
    format!(
        "/{proto}/{host}/udp/{}/quic-v1/p2p/{id}",
        ow_p2p::DEV_RELAY_PORT
    )
    .parse()
    .ok()
}

fn registry_url(host: &str) -> String {
    if host.contains(':') {
        format!("http://[{host}]:{REGISTRY_PORT}")
    } else {
        format!("http://{host}:{REGISTRY_PORT}")
    }
}

/// Host saved from the app.
#[must_use]
pub fn saved_host(config_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file(config_dir)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["host"]
        .as_str()
        .map(str::to_owned)
        .filter(|h| valid_host(h))
}

/// Saves (or clears) the host chosen in the app.
///
/// # Errors
///
/// File system errors.
pub fn save_host(config_dir: &Path, host: Option<&str>) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    match host {
        Some(h) => std::fs::write(
            file(config_dir),
            serde_json::json!({ "host": h }).to_string(),
        ),
        None => match std::fs::remove_file(file(config_dir)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

/// Resolves the configuration (see the module documentation).
#[must_use]
pub fn resolve(config_dir: &Path) -> Server {
    let host = saved_host(config_dir)
        .or_else(|| {
            option_env!("OW_DEFAULT_SERVER")
                .map(str::to_owned)
                .filter(|h| valid_host(h))
        })
        .or_else(|| cfg!(debug_assertions).then(|| "127.0.0.1".to_owned()));
    let registry = match std::env::var("OW_REGISTRY") {
        Ok(v) if v.eq_ignore_ascii_case("off") => None,
        Ok(v) => RegistryClient::new(&v).ok(),
        Err(_) => host
            .as_deref()
            .and_then(|h| RegistryClient::new(&registry_url(h)).ok()),
    };
    let relays = match std::env::var("OW_RELAY") {
        Ok(v) => v.split(',').filter_map(|a| a.trim().parse().ok()).collect(),
        Err(_) => host.as_deref().and_then(dev_relay).into_iter().collect(),
    };
    Server {
        host,
        registry,
        relays,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert!(valid_host("192.168.1.20"));
        assert!(valid_host("::1"));
        assert!(valid_host("pc-de-jean.local"));
        assert!(!valid_host(""));
        assert!(!valid_host("a b"));
        assert!(!valid_host("http://x"));
        assert!(
            dev_relay("192.168.1.20")
                .unwrap()
                .to_string()
                .starts_with("/ip4/192.168.1.20/udp/4001/")
        );
        assert!(
            dev_relay("pc.local")
                .unwrap()
                .to_string()
                .starts_with("/dns/pc.local/")
        );
        assert_eq!(registry_url("::1"), "http://[::1]:8787");
    }
}
