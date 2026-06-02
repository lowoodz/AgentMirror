use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use smr_core::{run_app, SharedApp, DEFAULT_CONFIG_YAML};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, RunEvent, Url, WindowEvent,
};
use tracing::info;

const TRAY_ID: &str = "main";

struct AppState {
    listen: String,
}

struct ServerHandle {
    shared: Arc<SharedApp>,
}

fn ui_url(listen: &str) -> String {
    format!("http://{listen}/ui")
}

fn parse_listen(listen: &str) -> Option<std::net::SocketAddr> {
    listen.parse().ok()
}

fn port_in_use(listen: &str) -> bool {
    let addr = match parse_listen(listen) {
        Some(a) => a,
        None => return false,
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

fn health_ready(listen: &str) -> bool {
    let addr = match parse_listen(listen) {
        Some(a) => a,
        None => return false,
    };
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(1)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let req = format!("GET /health HTTP/1.1\r\nHost: {listen}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 512];
    let Ok(n) = stream.read(&mut buf) else {
        return false;
    };
    let resp = String::from_utf8_lossy(&buf[..n]);
    resp.contains("200") && resp.contains("SafeRoute OK")
}

fn wait_for_server(listen: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if health_ready(listen) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn navigate_to_ui(window: &tauri::WebviewWindow, listen: &str) {
    let target = ui_url(listen);
    if let Ok(url) = Url::parse(&target) {
        let _ = window.navigate(url);
    }
}

fn show_boot_error(window: &tauri::WebviewWindow, listen: &str) {
    let msg = if port_in_use(listen) {
        format!(
            "端口 {listen} 已被占用，但服务无响应。\\n\
             请执行：\\n\
             launchctl unload ~/Library/LaunchAgents/com.securemodelroute.smr.plist\\n\
             pkill -f '/smr --config'\\n\
             然后重新打开 SafeRoute。"
        )
    } else {
        format!("无法启动 {listen} 上的服务。请在终端运行 smr 查看错误日志。")
    };
    let _ = window.eval(&format!(
        "document.getElementById('msg').textContent='服务未能启动';\
         document.getElementById('err').textContent='{msg}';\
         document.getElementById('spin').style.display='none';"
    ));
}

fn start_embedded_server(app: &tauri::AppHandle, listen: &str) {
    if health_ready(listen) {
        info!(listen = %listen, "reusing existing SafeRoute server");
        if let Some(window) = app.get_webview_window("main") {
            navigate_to_ui(&window, listen);
        }
        return;
    }

    if port_in_use(listen) {
        info!(listen = %listen, "port in use but health check failed");
        if let Some(window) = app.get_webview_window("main") {
            show_boot_error(&window, listen);
            let _ = window.show();
        }
        return;
    }

    let shared = app.state::<ServerHandle>().shared.clone();
    let app_handle = app.clone();
    let listen = listen.to_string();

    std::thread::spawn(move || {
        tauri::async_runtime::spawn(async move {
            if let Err(err) = run_app(shared).await {
                tracing::error!(error = %err, "server exited");
            }
        });

        if wait_for_server(&listen, Duration::from_secs(30)) {
            let listen = listen.clone();
            let app_for_ui = app_handle.clone();
            let _ = app_handle.run_on_main_thread(move || {
                if let Some(window) = app_for_ui.get_webview_window("main") {
                    navigate_to_ui(&window, &listen);
                }
            });
        } else {
            let listen = listen.clone();
            let app_for_ui = app_handle.clone();
            let _ = app_handle.run_on_main_thread(move || {
                if let Some(window) = app_for_ui.get_webview_window("main") {
                    show_boot_error(&window, &listen);
                    let _ = window.show();
                }
            });
        }
    });
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        if !health_ready(&state.listen) {
            start_embedded_server(app, &state.listen);
        } else if let Some(window) = app.get_webview_window("main") {
            navigate_to_ui(&window, &state.listen);
        }
    }
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
        .tooltip("SafeRoute — 点击打开主窗口")
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
            info!(config = %path.display(), listen = %listen, "starting SafeRoute server");

            app.manage(AppState {
                listen: listen.clone(),
            });
            app.manage(ServerHandle {
                shared: Arc::clone(&shared),
            });

            let window = app
                .get_webview_window("main")
                .ok_or("missing main window")?;

            let listen_js = listen.replace('\\', "\\\\").replace('\'', "\\'");
            let _ = window.eval(&format!("window.__SMR_LISTEN='{listen_js}';"));

            // Do not block setup — start server in background; bootstrap.html polls /health.
            start_embedded_server(app.handle(), &listen);

            if start_in_background() {
                let _ = window.hide();
            } else {
                let _ = window.show();
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build SafeRoute GUI");

    app.run(handle_run_event);
}

fn resolve_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("SMR_CONFIG") {
        return PathBuf::from(p);
    }
    smr_core::paths::default_config_path()
}
