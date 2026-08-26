#![forbid(unsafe_code)]

mod commands;

#[cfg(test)]
mod commands_test;

use std::error::Error;
use std::path::PathBuf;

use commands::DesktopState;
use pam_gui::DesktopCore;

#[cfg(target_os = "macos")]
use tauri::{Manager, TitleBarStyle};

/// Runs the local PAM Tauri application.
///
/// # Errors
///
/// Returns an error when the process environment cannot be resolved or Tauri
/// cannot start the platform webview runtime.
pub fn run() -> Result<(), Box<dyn Error>> {
    let startup_root = std::env::current_dir()?;
    let core = DesktopCore::with_daemon_executable(startup_root, daemon_executable()?);

    tauri::Builder::default()
        .manage(DesktopState::new(core))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                window.set_title_bar_style(TitleBarStyle::Overlay)?;
                window.set_title("")?;
            }

            #[cfg(not(target_os = "macos"))]
            let _ = app;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::catalog,
            commands::activate_project,
            commands::refresh_project,
            commands::start_daemon,
            commands::stop_daemon,
            commands::register_gui_caller,
            commands::decide_approval,
            commands::load_evidence,
            commands::load_flow_workspace,
            commands::daemon_activity,
            commands::daemon_logs,
            commands::daemon_stats,
            commands::caller_registry,
            commands::model_status,
            commands::model_infer,
            commands::model_import,
            commands::model_import_status,
            commands::model_inspect,
            commands::model_license_discover,
            commands::model_presets,
            commands::model_download,
            commands::model_download_status,
            commands::model_download_cancel,
            commands::host_memory,
            commands::app_settings,
            commands::settings_update,
            commands::logs_delete,
            commands::reveal_path,
            commands::flow_graph,
            commands::flow_compose,
            commands::load_skill_inventory,
            commands::manage_skill_library,
            commands::load_skill_audit,
            commands::run_skill_audit,
            commands::open_flow,
            commands::validate_flow,
            commands::save_flow,
            commands::connector_registry,
            commands::connector_configure,
            commands::connector_test,
            commands::daemon_health,
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}

fn daemon_executable() -> Result<PathBuf, std::io::Error> {
    let directory = std::env::current_exe()?
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::other("PAM executable has no parent directory"))?;
    Ok(directory.join(if cfg!(windows) { "pam.exe" } else { "pam" }))
}
