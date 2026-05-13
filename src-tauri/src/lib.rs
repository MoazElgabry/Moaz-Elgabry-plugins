mod catalog;
mod installer;
mod models;
mod settings;

use std::process::Command;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewWindow};

const SUPPORT_URL: &str = "https://buymeacoffee.com/moazelgabry";
const INITIAL_WINDOW_WIDTH: u32 = 1180;
const INITIAL_WINDOW_HEIGHT: u32 = 860;
const MIN_WINDOW_WIDTH: u32 = 900;
const MIN_WINDOW_HEIGHT: u32 = 760;
const STARTUP_WINDOW_MARGIN: i32 = 24;

#[tauri::command]
async fn dashboard_state() -> Result<models::DashboardState, String> {
    catalog::build_dashboard_state()
        .await
        .map_err(|error| models::UiError::from_error("dashboard", &error).to_json_string())
}

#[tauri::command]
async fn apply_plugin_action(
    plugin_id: String,
    action: String,
    target_version: Option<String>,
) -> Result<models::PluginOperationResult, String> {
    installer::apply_plugin_action(&plugin_id, &action, target_version.as_deref())
        .await
        .map_err(|error| models::UiError::from_error("plugin_action", &error).to_json_string())
}

#[tauri::command]
async fn set_beta_releases_enabled(enabled: bool) -> Result<(), String> {
    let mut current = settings::load_settings()
        .map_err(|error| models::UiError::from_error("settings", &error).to_json_string())?;
    current.beta_releases_enabled = enabled;
    settings::save_settings(&current)
        .map_err(|error| models::UiError::from_error("settings", &error).to_json_string())
}

#[tauri::command]
async fn export_plugin_logs(
    app: AppHandle,
    plugin_id: String,
    destination_dir: String,
    remove_previous_logs: bool,
) -> Result<models::PluginOperationResult, String> {
    installer::export_plugin_logs(&app, &plugin_id, &destination_dir, remove_previous_logs)
        .await
        .map_err(|error| models::UiError::from_error("export_logs", &error).to_json_string())
}

#[tauri::command]
async fn check_plugin_log_export_ready(plugin_id: String) -> Result<(), String> {
    installer::check_plugin_log_export_ready(&plugin_id)
        .await
        .map_err(|error| models::UiError::from_error("export_logs", &error).to_json_string())
}

#[tauri::command]
fn open_support_link() -> Result<(), String> {
    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("rundll32.exe");
        command.args(["url.dll,FileProtocolHandler", SUPPORT_URL]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(SUPPORT_URL);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(SUPPORT_URL);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Failed to open support link: {error}"))
}

fn place_startup_window(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()? else {
        return Ok(());
    };

    let work_area = monitor.work_area();
    let scale = monitor.scale_factor();
    let margin = (STARTUP_WINDOW_MARGIN as f64 * scale).round().max(0.0) as i32;
    let horizontal_margin = margin.saturating_mul(2).max(0) as u32;
    let vertical_margin = margin.saturating_mul(2).max(0) as u32;
    let target_width = scaled_dimension(INITIAL_WINDOW_WIDTH, scale)
        .min(work_area.size.width.saturating_sub(horizontal_margin))
        .max(MIN_WINDOW_WIDTH.min(work_area.size.width));
    let target_height = scaled_dimension(INITIAL_WINDOW_HEIGHT, scale)
        .min(work_area.size.height.saturating_sub(vertical_margin))
        .max(MIN_WINDOW_HEIGHT.min(work_area.size.height));
    let centered_x =
        work_area.position.x + ((work_area.size.width as i32 - target_width as i32) / 2).max(0);
    let top_y = work_area.position.y + margin;

    window.set_size(Size::Physical(PhysicalSize::new(
        target_width,
        target_height,
    )))?;
    window.set_position(Position::Physical(PhysicalPosition::new(centered_x, top_y)))?;
    Ok(())
}

fn scaled_dimension(value: u32, scale: f64) -> u32 {
    (value as f64 * scale).round().max(1.0) as u32
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin({
            let updater = tauri_plugin_updater::Builder::new();
            #[cfg(target_os = "macos")]
            let updater = updater.target("darwin-universal");
            updater.build()
        });

    builder
        .invoke_handler(tauri::generate_handler![
            dashboard_state,
            apply_plugin_action,
            export_plugin_logs,
            check_plugin_log_export_ready,
            set_beta_releases_enabled,
            open_support_link
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                place_startup_window(&window)?;
                window.show()?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Moaz Elgabry Plugins");
}
