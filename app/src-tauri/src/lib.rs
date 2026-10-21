//! OutpostWave client application (Tauri 2), shared by desktop and mobile.
//!
//! The UI talks to the Rust core through the commands below and receives `ow-event` events.
//!
//! Environment (development):
//! - `OW_PROFILE=<name>`: independent identity per profile (own data folder and database key),
//!   to test a conversation between two windows on one machine.
//! - `OW_REGISTRY=<url>|off` and `OW_RELAY=<multiaddr>[,...]`: override the test server chosen in
//!   the app (see [`server`]).

mod dto;
mod server;

use std::path::PathBuf;

use dto::{ContactDto, EventDto, InviteDto, MessageDto};
use ow_messaging::{
    ContactId, MessagingError, Messenger, NewProfile, ProfileSetup, RegistryClient, RegistryError,
};
use ow_storage::{Storage, keystore};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

/// Messages loaded per page.
const PAGE: u32 = 50;

/// Shared application state.
struct AppState {
    profile: String,
    data_dir: PathBuf,
    config_dir: PathBuf,
    server: std::sync::RwLock<server::Server>,
    messenger: Mutex<Option<Messenger>>,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Build information shown in the UI.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Client version.
    pub version: &'static str,
    /// Target OS (`windows`, `macos`, `linux`, `android`, `ios`).
    pub os: &'static str,
    /// Target CPU architecture (`x86_64`, `aarch64`, ...).
    pub arch: &'static str,
    /// Active profile.
    pub profile: String,
    /// An account server is configured (usernames, search, recovery).
    pub registry: bool,
    /// Test server host chosen in the app or at build time.
    pub server: Option<String>,
}

/// Returns build information about the running client.
#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        profile: state.profile.clone(),
        registry: state.server().registry.is_some(),
        server: state.server().host,
    }
}

impl AppState {
    fn open_storage(&self) -> CmdResult<Storage> {
        std::fs::create_dir_all(&self.data_dir).map_err(err)?;
        let key = keystore::load_or_create(&self.profile).map_err(err)?;
        Storage::open(&self.data_dir.join("outpostwave.db"), &key).map_err(err)
    }

    fn server(&self) -> server::Server {
        self.server.read().map(|s| s.clone()).unwrap_or_default()
    }

    fn registry(&self) -> CmdResult<RegistryClient> {
        self.server()
            .registry
            .ok_or_else(|| err(MessagingError::NoRegistry))
    }

    /// Starts the messenger on `storage` and forwards its events to the UI.
    fn start(&self, app: &AppHandle, storage: Storage) -> CmdResult<Messenger> {
        let server = self.server();
        let relays = server.relays;
        let (messenger, mut rx) =
            Messenger::start(storage, server.registry, |cfg| cfg.relays = relays).map_err(err)?;
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let _ = app.emit("ow-event", EventDto::from(event));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // The UI reloads its lists on this event.
                        let _ = app.emit("ow-event", EventDto::Resync);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(messenger)
    }

    /// Saves a prepared profile and starts the messenger.
    async fn finish(
        &self,
        app: &AppHandle,
        profile: NewProfile,
    ) -> CmdResult<(Option<Vec<String>>, String)> {
        let mut slot = self.messenger.lock().await;
        let storage = self.open_storage()?;
        let phrase = profile.commit(&storage).map_err(err)?;
        let words = phrase.map(|p| p.words().map(str::to_owned).collect());
        let m = self.start(app, storage)?;
        let handle = m.handle();
        *slot = Some(m);
        Ok((words, handle))
    }
}

async fn messenger(state: &State<'_, AppState>) -> CmdResult<Messenger> {
    state
        .messenger
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no identity".to_owned())
}

/// Returns our username if an identity exists (starting the messenger on first call).
#[tauri::command]
async fn identity_status(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let mut slot = state.messenger.lock().await;
    if let Some(m) = slot.as_ref() {
        return Ok(Some(m.handle()));
    }
    let storage = state.open_storage()?;
    if storage.load_local_identity().map_err(err)?.is_none() {
        return Ok(None);
    }
    let m = state.start(&app, storage)?;
    let handle = m.handle();
    *slot = Some(m);
    Ok(Some(handle))
}

fn registry_err(e: RegistryError) -> String {
    match e {
        RegistryError::Taken => "name_taken".to_owned(),
        other => err(other),
    }
}

/// Creates a new identity, claims the username on the registry, and returns the 24-word phrase.
#[tauri::command]
async fn create_identity(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    device_name: String,
    email: Option<String>,
    discoverable: bool,
) -> CmdResult<Vec<String>> {
    let mut profile = ProfileSetup::prepare_new(&name, &device_name).map_err(err)?;
    if let Some(reg) = &state.server().registry {
        let bundle = profile.bundle();
        let email = email.as_deref().map(str::trim).filter(|e| !e.is_empty());
        reg.register(
            profile.device(),
            profile.cert(),
            &bundle,
            profile.handle(),
            email,
            discoverable,
        )
        .await
        .map_err(registry_err)?;
    }
    let (words, _) = state.finish(&app, profile).await?;
    Ok(words.unwrap_or_default())
}

/// Adds this device to an existing identity with its recovery phrase.
#[tauri::command]
async fn restore_identity(
    app: AppHandle,
    state: State<'_, AppState>,
    phrase: String,
    name: String,
    device_name: String,
) -> CmdResult<String> {
    let mut profile = ProfileSetup::prepare_restore(&phrase, &name, &device_name).map_err(err)?;
    if let Some(reg) = &state.server().registry {
        let bundle = profile.bundle();
        reg.put_device(profile.device(), profile.cert(), &bundle)
            .await
            .map_err(registry_err)?;
        let account = reg.me(profile.device()).await.map_err(registry_err)?;
        if account.name != profile.handle().name() {
            return Err("name_mismatch".to_owned());
        }
    }
    Ok(state.finish(&app, profile).await?.1)
}

/// Step 1 of email recovery: sends a code to the account's verified email.
#[tauri::command]
async fn recovery_start(state: State<'_, AppState>, name: String) -> CmdResult<()> {
    let handle = ow_identity::Handle::new(&name).map_err(err)?;
    state
        .registry()?
        .recovery_start(&handle)
        .await
        .map_err(registry_err)
}

/// Step 2 of email recovery: creates new keys and re-binds the username to them.
/// Returns the new recovery phrase.
#[tauri::command]
async fn recovery_complete(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    code: String,
    device_name: String,
) -> CmdResult<Vec<String>> {
    let mut profile = ProfileSetup::prepare_new(&name, &device_name).map_err(err)?;
    let bundle = profile.bundle();
    state
        .registry()?
        .recovery_complete(
            profile.device(),
            profile.cert(),
            &bundle,
            profile.handle(),
            code.trim(),
        )
        .await
        .map_err(registry_err)?;
    let (words, _) = state.finish(&app, profile).await?;
    Ok(words.unwrap_or_default())
}

/// Our invite code and its QR code (SVG).
#[tauri::command]
async fn my_invite(state: State<'_, AppState>) -> CmdResult<InviteDto> {
    let m = messenger(&state).await?;
    let code = m.invite_code().map_err(err)?;
    let qr = qrcode::QrCode::new(code.as_bytes()).map_err(err)?;
    let qr_svg = qr
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(256, 256)
        .quiet_zone(true)
        .build();
    Ok(InviteDto {
        handle: m.handle(),
        code,
        qr_svg,
    })
}

/// Adds a contact from an invite code.
#[tauri::command]
async fn add_contact(state: State<'_, AppState>, code: String) -> CmdResult<ContactDto> {
    messenger(&state)
        .await?
        .add_contact(&code)
        .map(ContactDto::from)
        .map_err(err)
}

/// Adds a contact found by exact username.
#[tauri::command]
async fn add_contact_by_name(state: State<'_, AppState>, name: String) -> CmdResult<ContactDto> {
    match messenger(&state).await?.add_contact_by_name(&name).await {
        Ok(c) => Ok(ContactDto::from(c)),
        Err(MessagingError::Registry(RegistryError::NotFound)) => Err("user_not_found".to_owned()),
        Err(MessagingError::OwnInvite) => Err("own_account".to_owned()),
        Err(e) => Err(err(e)),
    }
}

/// Lists contacts.
#[tauri::command]
async fn list_contacts(state: State<'_, AppState>) -> CmdResult<Vec<ContactDto>> {
    let list = messenger(&state).await?.contacts().map_err(err)?;
    Ok(list.into_iter().map(ContactDto::from).collect())
}

/// Marks a contact's safety number as verified.
#[tauri::command]
async fn set_verified(state: State<'_, AppState>, contact: i64, verified: bool) -> CmdResult<()> {
    messenger(&state)
        .await?
        .set_verified(ContactId(contact), verified)
        .map_err(err)
}

/// Messages with a contact, newest first.
#[tauri::command]
async fn list_messages(
    state: State<'_, AppState>,
    contact: i64,
    before: Option<i64>,
) -> CmdResult<Vec<MessageDto>> {
    let list = messenger(&state)
        .await?
        .messages(ContactId(contact), before, PAGE)
        .map_err(err)?;
    Ok(list.iter().map(MessageDto::from).collect())
}

/// Sends a text message.
#[tauri::command]
async fn send_message(
    state: State<'_, AppState>,
    contact: i64,
    text: String,
) -> CmdResult<MessageDto> {
    let m = messenger(&state)
        .await?
        .send_text(ContactId(contact), &text)
        .map_err(err)?;
    Ok(MessageDto::from(&m))
}

/// Marks the conversation as read (sends read receipts if enabled).
#[tauri::command]
async fn mark_read(state: State<'_, AppState>, contact: i64) -> CmdResult<usize> {
    messenger(&state)
        .await?
        .mark_read(ContactId(contact))
        .map_err(err)
}

/// User settings. Account fields are `None` when the account server is unavailable.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    read_receipts: bool,
    discoverable: Option<bool>,
    email: Option<String>,
    email_verified: Option<bool>,
    #[serde(default)]
    backup_enabled: bool,
    /// The account server works but this identity has no account there (old profile).
    #[serde(default)]
    account_missing: bool,
    /// Notification sounds.
    #[serde(default = "yes")]
    sounds: bool,
}

const fn yes() -> bool {
    true
}

/// Reads the user settings.
#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    let m = messenger(&state).await?;
    let mut account_missing = false;
    let account = if state.server().registry.is_some() {
        match m.account().await {
            Ok(a) => Some(a),
            Err(MessagingError::Registry(RegistryError::UnknownDevice)) => {
                account_missing = true;
                None
            }
            Err(_) => None,
        }
    } else {
        None
    };
    Ok(Settings {
        read_receipts: m.read_receipts().map_err(err)?,
        discoverable: account.as_ref().map(|a| a.discoverable),
        email: account.as_ref().and_then(|a| a.email.clone()),
        email_verified: account.as_ref().map(|a| a.email_verified),
        backup_enabled: m.backup_enabled(),
        account_missing,
        sounds: m.pref("sounds", true).map_err(err)?,
    })
}

/// Saves the user settings (read receipts, search visibility).
#[tauri::command]
async fn save_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<()> {
    let m = messenger(&state).await?;
    m.set_read_receipts(settings.read_receipts).map_err(err)?;
    m.set_pref("sounds", settings.sounds).map_err(err)?;
    if let Some(d) = settings.discoverable {
        m.set_discoverable(d).await.map_err(err)?;
    }
    Ok(())
}

/// Sets (or removes, with an empty value) the recovery email. A code is emailed.
#[tauri::command]
async fn set_email(state: State<'_, AppState>, email: String) -> CmdResult<()> {
    let email = email.trim();
    messenger(&state)
        .await?
        .set_email((!email.is_empty()).then_some(email))
        .await
        .map_err(err)
}

/// Confirms the recovery email with the received code.
#[tauri::command]
async fn verify_email(state: State<'_, AppState>, code: String) -> CmdResult<()> {
    messenger(&state)
        .await?
        .verify_email(code.trim())
        .await
        .map_err(|e| match e {
            MessagingError::Registry(RegistryError::InvalidCode) => "invalid_code".to_owned(),
            other => err(other),
        })
}

fn backup_err(e: MessagingError) -> String {
    match e {
        MessagingError::Backup("wrong password") => "wrong_password".to_owned(),
        MessagingError::Backup("password too short") => "password_too_short".to_owned(),
        MessagingError::Backup("no backup on the server") => "no_backup".to_owned(),
        other => err(other),
    }
}

/// Enables encrypted backups with a password and uploads one now.
#[tauri::command]
async fn set_backup_password(state: State<'_, AppState>, password: String) -> CmdResult<()> {
    messenger(&state)
        .await?
        .set_backup_password(&password)
        .await
        .map_err(backup_err)
}

/// Uploads a backup now.
#[tauri::command]
async fn backup_now(state: State<'_, AppState>) -> CmdResult<()> {
    messenger(&state)
        .await?
        .backup_now()
        .await
        .map_err(backup_err)
}

/// Disables automatic backups on this device.
#[tauri::command]
async fn disable_backup(state: State<'_, AppState>) -> CmdResult<()> {
    messenger(&state).await?.disable_backup().map_err(err)
}

/// Restores contacts and history from the encrypted backup. Returns the number of contacts.
#[tauri::command]
async fn restore_backup(state: State<'_, AppState>, password: String) -> CmdResult<usize> {
    messenger(&state)
        .await?
        .restore_backup(&password)
        .await
        .map_err(backup_err)
}

/// Hides the "keys changed" warning of a contact.
#[tauri::command]
async fn ack_key_change(state: State<'_, AppState>, contact: i64) -> CmdResult<()> {
    messenger(&state)
        .await?
        .ack_key_change(ContactId(contact))
        .map_err(err)
}

/// Deletes the account on the account server (username, email, backup).
#[tauri::command]
async fn delete_account(state: State<'_, AppState>) -> CmdResult<()> {
    messenger(&state).await?.delete_account().await.map_err(err)
}

/// Local UI preferences (no network access, unlike [`get_settings`]).
#[derive(Debug, Serialize)]
struct Prefs {
    sounds: bool,
}

/// Reads the local UI preferences.
#[tauri::command]
async fn get_prefs(state: State<'_, AppState>) -> CmdResult<Prefs> {
    let m = messenger(&state).await?;
    Ok(Prefs {
        sounds: m.pref("sounds", true).map_err(err)?,
    })
}

/// Claims the username on the account server for a profile created before it existed.
#[tauri::command]
async fn register_account(
    state: State<'_, AppState>,
    email: Option<String>,
    discoverable: bool,
) -> CmdResult<()> {
    let email = email.as_deref().map(str::trim).filter(|e| !e.is_empty());
    messenger(&state)
        .await?
        .register_account(email, discoverable)
        .await
        .map_err(|e| match e {
            MessagingError::Registry(r) => registry_err(r),
            other => err(other),
        })
}

/// Unread count on the taskbar icon. On Windows, `overlay` is the 32×32 RGBA badge drawn by the
/// UI (Windows has no numeric badge API); elsewhere the native badge count is used.
#[tauri::command]
fn set_badge(app: AppHandle, count: u32, overlay: Option<Vec<u8>>) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    #[cfg(target_os = "windows")]
    {
        let icon = overlay
            .filter(|o| count > 0 && o.len() == 32 * 32 * 4)
            .map(|o| tauri::image::Image::new_owned(o, 32, 32));
        let _ = w.set_overlay_icon(icon);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = overlay;
        let _ = w.set_badge_count((count > 0).then_some(i64::from(count)));
    }
}

/// Flashes the taskbar entry when the window is not focused.
#[tauri::command]
fn request_attention(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if !w.is_focused().unwrap_or(true) {
            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
        }
    }
}

/// Sets (or clears, with `None`) the test server host, then restarts the app if an identity is
/// already running.
#[tauri::command]
async fn set_server(
    app: AppHandle,
    state: State<'_, AppState>,
    host: Option<String>,
) -> CmdResult<()> {
    let host = host.map(|h| h.trim().to_owned()).filter(|h| !h.is_empty());
    if let Some(h) = &host {
        if !server::valid_host(h) {
            return Err("invalid_host".to_owned());
        }
    }
    server::save_host(&state.config_dir, host.as_deref()).map_err(err)?;
    if state.messenger.lock().await.is_some() {
        app.restart();
    }
    if let Ok(mut s) = state.server.write() {
        *s = server::resolve(&state.config_dir);
    }
    Ok(())
}

/// Profile name from `OW_PROFILE`, restricted to `[a-z0-9_-]{1,32}`.
fn profile_name() -> String {
    std::env::var("OW_PROFILE")
        .ok()
        .map(|p| p.to_ascii_lowercase())
        .filter(|p| {
            (1..=32).contains(&p.len())
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .unwrap_or_else(|| "default".to_owned())
}

/// Starts the application.
///
/// # Panics
///
/// Panics if the Tauri runtime fails to start.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let profile = profile_name();
            let data_dir = app.path().app_data_dir()?.join("profiles").join(&profile);
            if profile != "default" {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.set_title(&format!("OutpostWave — {profile}"));
                }
            }
            let config_dir = app.path().app_config_dir()?;
            let server = std::sync::RwLock::new(server::resolve(&config_dir));
            app.manage(AppState {
                profile,
                data_dir,
                config_dir,
                server,
                messenger: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            identity_status,
            create_identity,
            restore_identity,
            recovery_start,
            recovery_complete,
            my_invite,
            add_contact,
            add_contact_by_name,
            list_contacts,
            set_verified,
            list_messages,
            send_message,
            mark_read,
            get_settings,
            save_settings,
            set_email,
            verify_email,
            set_backup_password,
            backup_now,
            disable_backup,
            restore_backup,
            ack_key_change,
            delete_account,
            register_account,
            set_badge,
            request_attention,
            set_server,
            get_prefs
        ])
        .run(tauri::generate_context!())
        .expect("failed to start OutpostWave");
}
