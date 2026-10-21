//! OutpostWave client application (Tauri 2), shared by desktop and mobile.

use serde::Serialize;

/// Build information shown in the UI.
#[derive(Debug, Serialize)]
pub struct AppInfo {
    /// Client version.
    pub version: &'static str,
    /// Target OS (`windows`, `macos`, `linux`, `android`, `ios`).
    pub os: &'static str,
    /// Target CPU architecture (`x86_64`, `aarch64`, ...).
    pub arch: &'static str,
}

/// Returns build information about the running client.
#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    }
}

/// Starts the application.
///
/// # Panics
///
/// Panics if the Tauri runtime fails to start.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info])
        .run(tauri::generate_context!())
        .expect("failed to start OutpostWave");
}
