//! OutpostWave client application (Tauri 2), shared by desktop and mobile.
//!
//! The UI talks to the Rust core through the commands below and receives `ow-event` events.
//!
//! Profiles: set `OW_PROFILE=<name>` to run several independent identities on one machine
//! (useful to test a conversation between two windows). Each profile has its own data folder and
//! database key.

mod dto;

use std::path::PathBuf;

use dto::{ContactDto, EventDto, InviteDto, MessageDto};
use ow_messaging::{ContactId, Messenger, ProfileSetup};
use ow_storage::{Storage, keystore};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

/// Messages loaded per page.
const PAGE: u32 = 50;

/// Shared application state.
struct AppState {
    profile: String,
    data_dir: PathBuf,
    messenger: Mutex<Option<Messenger>>,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Build information shown in the UI.
#[derive(Debug, Serialize)]
pub struct AppInfo {
    /// Client version.
    pub version: &'static str,
    /// Target OS (`windows`, `macos`, `linux`, `android`, `ios`).
    pub os: &'static str,
    /// Target CPU architecture (`x86_64`, `aarch64`, ...).
    pub arch: &'static str,
    /// Active profile.
    pub profile: String,
}

/// Returns build information about the running client.
#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        profile: state.profile.clone(),
    }
}

impl AppState {
    fn open_storage(&self) -> CmdResult<Storage> {
        std::fs::create_dir_all(&self.data_dir).map_err(err)?;
        let key = keystore::load_or_create(&self.profile).map_err(err)?;
        Storage::open(&self.data_dir.join("outpostwave.db"), &key).map_err(err)
    }

    /// Starts the messenger on `storage` and forwards its events to the UI.
    fn start(&self, app: &AppHandle, storage: Storage) -> CmdResult<Messenger> {
        let (messenger, mut rx) = Messenger::start(storage, |_| {}).map_err(err)?;
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
}

async fn messenger(state: &State<'_, AppState>) -> CmdResult<Messenger> {
    state
        .messenger
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no identity".to_owned())
}

/// Returns our handle if an identity exists (starting the messenger on first call).
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

/// Creates a new identity and returns the 24-word recovery phrase.
#[tauri::command]
async fn create_identity(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    device_name: String,
) -> CmdResult<Vec<String>> {
    let mut slot = state.messenger.lock().await;
    let storage = state.open_storage()?;
    let phrase = ProfileSetup::create(&storage, &name, &device_name).map_err(err)?;
    let words = phrase.words().map(str::to_owned).collect();
    *slot = Some(state.start(&app, storage)?);
    Ok(words)
}

/// Restores an identity from its recovery phrase.
#[tauri::command]
async fn restore_identity(
    app: AppHandle,
    state: State<'_, AppState>,
    phrase: String,
    name: String,
    device_name: String,
) -> CmdResult<String> {
    let mut slot = state.messenger.lock().await;
    let storage = state.open_storage()?;
    ProfileSetup::restore(&storage, &phrase, &name, &device_name).map_err(err)?;
    let m = state.start(&app, storage)?;
    let handle = m.handle();
    *slot = Some(m);
    Ok(handle)
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
            app.manage(AppState {
                profile,
                data_dir,
                messenger: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            identity_status,
            create_identity,
            restore_identity,
            my_invite,
            add_contact,
            list_contacts,
            set_verified,
            list_messages,
            send_message
        ])
        .run(tauri::generate_context!())
        .expect("failed to start OutpostWave");
}
