use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use smr_core::{run_app, SharedApp, DEFAULT_CONFIG_YAML};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, WindowEvent,
};
use tracing::info;

const TRAY_ID: &str = "main";

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

fn main_window_visible(app: &tauri::AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

fn start_in_background() -> bool {
    std::env::args().any(|arg| arg == "--background" || arg == "--tray-only")
}

fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let icon = app
        .default_window_icon()
        .ok_or("missing default window icon")?
        .clone();

    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .tooltip("SecureModelRoute — 点击打开主窗口")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    #[cfg(target_os = "macos")]
    let builder = builder.icon_as_template(true);

    builder.build(app)?;
    Ok(())
}

fn handle_run_event(app_handle: &tauri::AppHandle, event: RunEvent) {
    match event {
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main_window(app_handle),
        RunEvent::ExitRequested { api, code, .. } => {
            // macOS: closing the last window can request app exit; hide instead while visible.
            // When the window is already hidden, allow Cmd+Q / dock Quit to exit.
            if code.is_none() && main_window_visible(app_handle) {
                api.prevent_exit();
                hide_main_window(app_handle);
            }
        }
        _ => {}
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            setup_tray(app).map_err(|e| e.to_string())?;

            let config_path = resolve_config_path();
            let (shared, path) = SharedApp::load_or_create(&config_path, DEFAULT_CONFIG_YAML)
                .map_err(|e| format!("config error: {e}"))?;
            let listen = shared.config().server.listen.clone();
            info!(config = %path.display(), listen = %listen, "starting SecureModelRoute server");

            let shared = Arc::clone(&shared);
            tauri::async_runtime::spawn(async move {
                if let Err(err) = run_app(shared).await {
                    tracing::error!(error = %err, "server exited");
                }
            });

            std::thread::sleep(Duration::from_millis(600));

            if let Some(window) = app.get_webview_window("main") {
                let ui = format!("http://{listen}/ui");
                let _ = window.eval(&format!("window.location.replace('{ui}')"));
                if start_in_background() {
                    let _ = window.hide();
                }
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build SecureModelRoute GUI");

    app.run(handle_run_event);
}

fn resolve_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("SMR_CONFIG") {
        return PathBuf::from(p);
    }
    smr_core::paths::default_config_path()
}
