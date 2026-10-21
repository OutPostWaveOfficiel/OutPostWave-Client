//! Client of the OutpostWave account registry (`spec/v0/registry.md` in OutpostWave-Protocol).
//!
//! Every private or mutating request is signed with the device key. Answers from the registry
//! are **verified** (certificates and bundles must be correctly signed and belong to the returned
//! identity), so a registry can at worst lie about *which* identity owns a name — something users
//! detect with safety numbers.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ow_crypto::PrekeyBundle;
use ow_identity::{DeviceCertificate, DeviceKey, Handle, PublicKey};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

/// Signing context of registry requests.
const CONTEXT: &[u8] = b"OutpostWave registry request v1\0";

/// Registry errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The registry cannot be reached.
    Unreachable(String),
    /// The username is already taken (or the identity already has an account).
    Taken,
    /// Not found.
    NotFound,
    /// Invalid or expired code.
    InvalidCode,
    /// Too many requests: retry later.
    RateLimited,
    /// The registry rejected the request.
    Rejected(String),
    /// The registry answered with data that does not verify.
    InvalidResponse,
}

impl core::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unreachable(e) => write!(f, "account server unreachable: {e}"),
            Self::Taken => f.write_str("username already taken"),
            Self::NotFound => f.write_str("user not found"),
            Self::InvalidCode => f.write_str("invalid or expired code"),
            Self::RateLimited => f.write_str("too many attempts, try again later"),
            Self::Rejected(m) => write!(f, "account server refused: {m}"),
            Self::InvalidResponse => f.write_str("account server sent invalid data"),
        }
    }
}

impl std::error::Error for RegistryError {}

type Result<T> = core::result::Result<T, RegistryError>;

/// A user found in the registry, with verified devices.
#[derive(Debug, Clone)]
pub struct RemoteAccount {
    /// Username.
    pub name: Handle,
    /// Master public key.
    pub master: PublicKey,
    /// Devices: certificate and prekey bundle, both verified.
    pub devices: Vec<(DeviceCertificate, PrekeyBundle)>,
}

/// Account settings as stored by the registry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    /// Username.
    pub name: String,
    /// Email, if any.
    pub email: Option<String>,
    /// Email verified.
    pub email_verified: bool,
    /// Findable by username.
    pub discoverable: bool,
}

/// Registry client.
#[derive(Debug, Clone)]
pub struct RegistryClient {
    base: String,
    http: reqwest::Client,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Last timestamp used, to keep signed timestamps strictly increasing (replay protection).
static LAST_TS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn next_ts() -> i64 {
    let n = now();
    let prev = LAST_TS.fetch_max(n, std::sync::atomic::Ordering::SeqCst);
    if prev >= n {
        LAST_TS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    } else {
        n
    }
}

impl RegistryClient {
    /// Creates a client for the registry at `base_url` (e.g. `https://accounts.outpostwave.app`).
    ///
    /// # Errors
    ///
    /// [`RegistryError::Unreachable`] if the HTTP client cannot be built.
    pub fn new(base_url: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("outpostwave/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| RegistryError::Unreachable(e.to_string()))?;
        Ok(Self {
            base: base_url.trim_end_matches('/').to_owned(),
            http,
        })
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Vec<u8>,
        signer: Option<&DeviceKey>,
        content_type: &str,
    ) -> Result<reqwest::Response> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .header("content-type", content_type);
        if let Some(device) = signer {
            let ts = next_ts();
            let mut msg = CONTEXT.to_vec();
            for part in [
                method.as_str().as_bytes(),
                path.as_bytes(),
                ts.to_string().as_bytes(),
            ] {
                msg.extend_from_slice(part);
                msg.push(0);
            }
            msg.extend_from_slice(&body);
            req = req
                .header("x-ow-key", B64.encode(device.public_key().to_bytes()))
                .header("x-ow-timestamp", ts.to_string())
                .header("x-ow-signature", B64.encode(device.sign(&msg)));
        }
        let res = req
            .body(body)
            .send()
            .await
            .map_err(|e| RegistryError::Unreachable(e.to_string()))?;
        match res.status() {
            s if s.is_success() => Ok(res),
            StatusCode::CONFLICT => Err(RegistryError::Taken),
            StatusCode::NOT_FOUND => Err(RegistryError::NotFound),
            StatusCode::TOO_MANY_REQUESTS => Err(RegistryError::RateLimited),
            _ => {
                let v: Value = res.json().await.unwrap_or(Value::Null);
                let msg = v["message"].as_str().unwrap_or("error").to_owned();
                Err(if msg.contains("code") {
                    RegistryError::InvalidCode
                } else {
                    RegistryError::Rejected(msg)
                })
            }
        }
    }

    async fn json(
        &self,
        method: Method,
        path: &str,
        body: &Value,
        signer: Option<&DeviceKey>,
    ) -> Result<reqwest::Response> {
        let bytes = serde_json::to_vec(body).map_err(|_| RegistryError::InvalidResponse)?;
        self.send(method, path, bytes, signer, "application/json")
            .await
    }

    /// Registers a new account for the identity of `cert`.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Taken`] if the name is taken, or other errors.
    pub async fn register(
        &self,
        device: &DeviceKey,
        cert: &DeviceCertificate,
        bundle: &PrekeyBundle,
        name: &Handle,
        email: Option<&str>,
        discoverable: bool,
    ) -> Result<()> {
        let body = json!({
            "name": name.name(),
            "deviceCert": B64.encode(cert.to_bytes()),
            "bundle": B64.encode(bundle.to_bytes()),
            "email": email,
            "discoverable": discoverable,
        });
        self.json(Method::POST, "/v1/accounts", &body, Some(device))
            .await
            .map(|_| ())
    }

    /// Looks up a user by exact username. Returns [`RegistryError::NotFound`] if the user does not
    /// exist or is not discoverable.
    ///
    /// # Errors
    ///
    /// [`RegistryError::NotFound`], [`RegistryError::InvalidResponse`] or network errors.
    pub async fn lookup(&self, name: &Handle) -> Result<RemoteAccount> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Dev {
            cert: String,
            bundle: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Resp {
            name: String,
            master_pub: String,
            devices: Vec<Dev>,
        }
        let res = self
            .send(
                Method::GET,
                &format!("/v1/accounts/{}", name.name()),
                Vec::new(),
                None,
                "application/json",
            )
            .await?;
        let r: Resp = res
            .json()
            .await
            .map_err(|_| RegistryError::InvalidResponse)?;
        let bad = || RegistryError::InvalidResponse;
        let returned = Handle::new(&r.name).map_err(|_| bad())?;
        if &returned != name {
            return Err(bad());
        }
        let master: [u8; 32] = B64
            .decode(&r.master_pub)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(bad)?;
        let master = PublicKey::from_bytes(&master).map_err(|_| bad())?;
        let mut devices = Vec::new();
        for d in r.devices {
            let cert = DeviceCertificate::from_bytes(&B64.decode(&d.cert).map_err(|_| bad())?)
                .map_err(|_| bad())?;
            if cert.master() != &master {
                return Err(bad());
            }
            let bundle =
                PrekeyBundle::from_bytes(&B64.decode(&d.bundle).map_err(|_| bad())?, cert.device())
                    .map_err(|_| bad())?;
            devices.push((cert, bundle));
        }
        if devices.is_empty() {
            return Err(bad());
        }
        Ok(RemoteAccount {
            name: returned,
            master,
            devices,
        })
    }

    /// Our account settings.
    ///
    /// # Errors
    ///
    /// Network or authentication errors.
    pub async fn me(&self, device: &DeviceKey) -> Result<AccountInfo> {
        let res = self
            .send(
                Method::GET,
                "/v1/me",
                Vec::new(),
                Some(device),
                "application/json",
            )
            .await?;
        res.json().await.map_err(|_| RegistryError::InvalidResponse)
    }

    /// Shows or hides the account in username search.
    ///
    /// # Errors
    ///
    /// Network or authentication errors.
    pub async fn set_discoverable(&self, device: &DeviceKey, discoverable: bool) -> Result<()> {
        self.json(
            Method::PUT,
            "/v1/me/settings",
            &json!({ "discoverable": discoverable }),
            Some(device),
        )
        .await
        .map(|_| ())
    }

    /// Sets (or removes, with `None`) the recovery email; a code is sent to verify it.
    ///
    /// # Errors
    ///
    /// Network or authentication errors.
    pub async fn set_email(&self, device: &DeviceKey, email: Option<&str>) -> Result<()> {
        self.json(
            Method::PUT,
            "/v1/me/email",
            &json!({ "email": email }),
            Some(device),
        )
        .await
        .map(|_| ())
    }

    /// Confirms the email with the received code.
    ///
    /// # Errors
    ///
    /// [`RegistryError::InvalidCode`] or network errors.
    pub async fn verify_email(&self, device: &DeviceKey, code: &str) -> Result<()> {
        self.json(
            Method::POST,
            "/v1/me/email/verify",
            &json!({ "code": code }),
            Some(device),
        )
        .await
        .map(|_| ())
    }

    /// Adds this device to the account of its identity (restore with the recovery phrase), or
    /// refreshes its prekey bundle.
    ///
    /// # Errors
    ///
    /// [`RegistryError::Rejected`] if the identity has no account, or network errors.
    pub async fn put_device(
        &self,
        device: &DeviceKey,
        cert: &DeviceCertificate,
        bundle: &PrekeyBundle,
    ) -> Result<()> {
        let body = json!({
            "deviceCert": B64.encode(cert.to_bytes()),
            "bundle": B64.encode(bundle.to_bytes()),
        });
        self.json(Method::PUT, "/v1/me/device", &body, Some(device))
            .await
            .map(|_| ())
    }

    /// Uploads the encrypted backup.
    ///
    /// # Errors
    ///
    /// Network or authentication errors.
    pub async fn put_backup(&self, device: &DeviceKey, blob: Vec<u8>) -> Result<()> {
        self.send(
            Method::PUT,
            "/v1/me/backup",
            blob,
            Some(device),
            "application/octet-stream",
        )
        .await
        .map(|_| ())
    }

    /// Downloads the encrypted backup.
    ///
    /// # Errors
    ///
    /// [`RegistryError::NotFound`] if there is none, or network errors.
    pub async fn get_backup(&self, device: &DeviceKey) -> Result<Vec<u8>> {
        let res = self
            .send(
                Method::GET,
                "/v1/me/backup",
                Vec::new(),
                Some(device),
                "application/octet-stream",
            )
            .await?;
        res.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| RegistryError::Unreachable(e.to_string()))
    }

    /// Asks for a recovery code by email.
    ///
    /// # Errors
    ///
    /// Network errors.
    pub async fn recovery_start(&self, name: &Handle) -> Result<()> {
        self.json(
            Method::POST,
            "/v1/recovery/start",
            &json!({ "name": name.name() }),
            None,
        )
        .await
        .map(|_| ())
    }

    /// Completes recovery: binds the username to this new device's identity.
    ///
    /// # Errors
    ///
    /// [`RegistryError::InvalidCode`] or network errors.
    pub async fn recovery_complete(
        &self,
        device: &DeviceKey,
        cert: &DeviceCertificate,
        bundle: &PrekeyBundle,
        name: &Handle,
        code: &str,
    ) -> Result<()> {
        let body = json!({
            "name": name.name(),
            "code": code,
            "deviceCert": B64.encode(cert.to_bytes()),
            "bundle": B64.encode(bundle.to_bytes()),
        });
        self.json(Method::POST, "/v1/recovery/complete", &body, Some(device))
            .await
            .map(|_| ())
    }

    /// Deletes the account on the registry.
    ///
    /// # Errors
    ///
    /// Network or authentication errors.
    pub async fn delete_account(&self, device: &DeviceKey) -> Result<()> {
        self.send(
            Method::DELETE,
            "/v1/me",
            Vec::new(),
            Some(device),
            "application/json",
        )
        .await
        .map(|_| ())
    }
}
