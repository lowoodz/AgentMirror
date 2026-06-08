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

fn fetch_health_body(listen: &str) -> Option<String> {
    let addr = parse_listen(listen)?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let req = format!("GET /health HTTP/1.1\r\nHost: {listen}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    let mut buf = [0u8; 512];
    let n = stream.read(&mut buf).ok()?;
    let resp = String::from_utf8_lossy(&buf[..n]);
    if !resp.contains("200") || !resp.contains("LLM-SafeRoute OK") {
        return None;
    }
    resp.split("\r\n\r\n")
        .nth(1)
        .map(|body| body.trim().to_string())
}

fn force_fresh_server() -> bool {
    std::env::var("SMR_FORCE_SERVER")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HealthInfo {
    version: Option<String>,
    ui_digest: Option<String>,
}

fn parse_health_body(body: &str) -> Option<HealthInfo> {
    let tail = body.split("LLM-SafeRoute OK").nth(1)?.trim();
    let mut parts = tail.split_whitespace();
    let version = parts
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut ui_digest = None;
    for part in parts {
        if let Some(digest) = part.strip_prefix("ui=") {
            if !digest.is_empty() {
                ui_digest = Some(digest.to_string());
            }
        }
    }
    Some(HealthInfo {
        version,
        ui_digest,
    })
}

fn health_info(listen: &str) -> Option<HealthInfo> {
    let body = fetch_health_body(listen)?;
    parse_health_body(&body)
}

fn health_ready(listen: &str) -> bool {
    health_info(listen).is_some()
}

fn health_matches_app(listen: &str) -> bool {
    let Some(info) = health_info(listen) else {
        return false;
    };
    info.version.as_deref() == Some(env!("CARGO_PKG_VERSION"))
        && info.ui_digest.as_deref() == Some(smr_core::UI_DIGEST)
}

fn wait_for_server(listen: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if health_matches_app(listen) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn navigate_to_ui(window: &tauri::WebviewWindow, listen: &str) {
    let target = ui_url(listen);
    if window
        .url()
        .ok()
        .is_some_and(|current| current.as_str() == target)
    {
        return;
    }
    if let Ok(url) = Url::parse(&target) {
        let _ = window.navigate(url);
    }
}

fn show_boot_error(window: &tauri::WebviewWindow, listen: &str) {
    let msg = if port_in_use(listen) {
        #[cfg(windows)]
        let steps = "请退出托盘中的 SafeRoute，或在任务管理器中结束 smr.exe / SafeRoute.exe，然后重新打开。";
        #[cfg(not(windows))]
        let steps = "请执行：\\n\
             launchctl unload ~/Library/LaunchAgents/com.securemodelroute.smr.plist\\n\
             pkill -f '/smr --config'\\n\
             然后重新打开 LLM-SafeRoute。";
        format!("端口 {listen} 已被占用，但服务无响应。\\n{steps}")
    } else {
        format!("无法启动 {listen} 上的服务。请在终端运行 smr 查看错误日志。")
    };
    let _ = window.eval(&format!(
        "document.getElementById('msg').textContent='服务未能启动';\
         document.getElementById('err').textContent='{msg}';\
         document.getElementById('spin').style.display='none';"
    ));
}

fn show_stale_server_error(
    window: &tauri::WebviewWindow,
    listen: &str,
    remote_version: Option<&str>,
    remote_ui: Option<&str>,
) {
    let remote_label = remote_version.unwrap_or("未知版本");
    let local = env!("CARGO_PKG_VERSION");
    let reason = if remote_version == Some(local) {
        "管理界面版本过旧（端口被旧 smr 进程占用）"
    } else {
        "检测到旧版服务"
    };
    #[cfg(windows)]
    let steps = format!(
        "请在任务管理器中结束 smr.exe / SafeRoute.exe，\\n\
         然后重新打开 LLM-SafeRoute（当前 {local}）。"
    );
    #[cfg(not(windows))]
    let steps = format!(
        "请执行：\\n\
         launchctl unload ~/Library/LaunchAgents/com.securemodelroute.smr.plist\\n\
         pkill -f '/smr --config'\\n\
         然后重新打开 LLM-SafeRoute（当前 {local}）。"
    );
    let ui_note = match remote_ui {
        Some(ui) if !ui.is_empty() => format!("（UI {ui}）"),
        _ => String::new(),
    };
    let msg = format!(
        "端口 {listen} 上已有旧版服务 {remote_label}{ui_note}。\\n{steps}"
    );
    let _ = window.eval(&format!(
        "document.getElementById('msg').textContent='{reason}';\
         document.getElementById('err').textContent='{msg}';\
         document.getElementById('spin').style.display='none';"
    ));
}

fn start_embedded_server(app: &tauri::AppHandle, listen: &str) {
    if !force_fresh_server() && health_matches_app(listen) {
        info!(listen = %listen, "reusing existing LLM-SafeRoute server");
        if let Some(window) = app.get_webview_window("main") {
            navigate_to_ui(&window, listen);
        }
        return;
    }

    if health_ready(listen) && !health_matches_app(listen) {
        let remote = health_info(listen);
        let remote_version = remote.as_ref().and_then(|i| i.version.as_deref());
        let remote_ui = remote.as_ref().and_then(|i| i.ui_digest.as_deref());
        info!(
            listen = %listen,
            remote = remote_version.unwrap_or("unknown"),
            remote_ui = remote_ui.unwrap_or("missing"),
            local = env!("CARGO_PKG_VERSION"),
            local_ui = smr_core::UI_DIGEST,
            "refusing to reuse stale LLM-SafeRoute server"
        );
        if let Some(window) = app.get_webview_window("main") {
            show_stale_server_error(&window, listen, remote_version, remote_ui);
            let _ = window.show();
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
        if health_matches_app(&state.listen) {
            if let Some(window) = app.get_webview_window("main") {
                navigate_to_ui(&window, &state.listen);
            }
        } else {
            start_embedded_server(app, &state.listen);
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        apply_window_chrome(&window);
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

const WINDOW_BG: (u8, u8, u8) = (0x2e, 0x32, 0x3a);

#[cfg(windows)]
const WINDOW_TEXT: (u8, u8, u8) = (0xee, 0xf1, 0xf6);

#[cfg(windows)]
fn rgb_to_colorref((r, g, b): (u8, u8, u8)) -> u32 {
    u32::from(b) << 16 | u32::from(g) << 8 | u32::from(r)
}

#[cfg(target_os = "macos")]
fn set_macos_window_background(raw: *mut std::ffi::c_void) {
    use objc2_app_kit::{NSColor, NSWindow};

    let ns_window = raw as *mut NSWindow;
    let ns_window = unsafe { &*ns_window };
    let (r, g, b) = WINDOW_BG;
    let bg = NSColor::colorWithRed_green_blue_alpha(
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0,
        1.0,
    );
    ns_window.setBackgroundColor(Some(&bg));
}

#[cfg(windows)]
fn apply_windows_native_chrome(raw: *mut std::ffi::c_void) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SendMessageW, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
        GWL_EXSTYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        WM_SETICON, WS_EX_DLGMODALFRAME,
    };

    let hwnd: HWND = raw;
    let caption = rgb_to_colorref(WINDOW_BG);
    let border = caption;
    let text = rgb_to_colorref(WINDOW_TEXT);
    let dark_mode: u32 = 1;
    let size = std::mem::size_of::<u32>() as u32;

    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark_mode as *const u32).cast(),
            size,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            (&caption as *const u32).cast(),
            size,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            (&border as *const u32).cast(),
            size,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR as u32,
            (&text as *const u32).cast(),
            size,
        );

        // In-app header already shows branding; keep the native caption area minimal.
        let _ = SetWindowTextW(hwnd, [0u16].as_ptr());
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            ex_style | WS_EX_DLGMODALFRAME as isize,
        );
        let _ = SendMessageW(hwnd, WM_SETICON, 0, 0);
        let _ = SendMessageW(hwnd, WM_SETICON, 1, 0);

        let _ = SetWindowPos(
            hwnd,
            0 as HWND,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

fn refresh_native_titlebar(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        use tauri::TitleBarStyle;

        let _ = window.set_title_bar_style(TitleBarStyle::Transparent);
        if let Ok(raw) = window.ns_window() {
            set_macos_window_background(raw);
        }
    }

    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        apply_windows_native_chrome(hwnd.0 as *mut std::ffi::c_void);
    }
}

fn refresh_native_titlebar_for_window(window: &tauri::Window) {
    #[cfg(target_os = "macos")]
    if let Ok(raw) = window.ns_window() {
        set_macos_window_background(raw);
    }

    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        apply_windows_native_chrome(hwnd.0 as *mut std::ffi::c_void);
    }
}

fn apply_window_chrome(window: &tauri::WebviewWindow) {
    use tauri::{Theme, window::Color};

    let (r, g, b) = WINDOW_BG;
    let _ = window.set_theme(Some(Theme::Dark));
    let _ = window.set_background_color(Some(Color(r, g, b, 255)));
    #[cfg(windows)]
    let _ = window.set_title("");
    refresh_native_titlebar(window);
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
        .tooltip("LLM-SafeRoute — 点击打开主窗口")
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
                return;
            }
            if window.label() == "main"
                && matches!(
                    event,
                    WindowEvent::Resized(_)
                        | WindowEvent::ScaleFactorChanged { .. }
                        | WindowEvent::Focused(_)
                )
            {
                refresh_native_titlebar_for_window(window);
            }
        })
        .setup(|app| {
            setup_tray(app).map_err(|e| e.to_string())?;

            let config_path = resolve_config_path();
            let (shared, path) = SharedApp::load_or_create(&config_path, DEFAULT_CONFIG_YAML)
                .map_err(|e| format!("config error: {e}"))?;
            let listen = shared.config().server.listen.clone();
            info!(config = %path.display(), listen = %listen, "starting LLM-SafeRoute server");

            app.manage(AppState {
                listen: listen.clone(),
            });
            app.manage(ServerHandle {
                shared: Arc::clone(&shared),
            });

            let window = app
                .get_webview_window("main")
                .ok_or("missing main window")?;

            apply_window_chrome(&window);

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
        .expect("failed to build LLM-SafeRoute GUI");

    app.run(handle_run_event);
}

fn resolve_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("SMR_CONFIG") {
        return PathBuf::from(p);
    }
    smr_core::paths::default_config_path()
}
