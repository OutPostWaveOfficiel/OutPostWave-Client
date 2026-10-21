//! OutpostWave desktop entry point.

// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ow_app_lib::run();
}
