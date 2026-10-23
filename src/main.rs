mod config;
mod icon_gen;
mod macos;
mod selftest;

use config::{
    build_folder_app_bundles, get_cached_app_icon, load_or_init_config,
    pin_app_to_dock, remove_from_dock,
    restore_macos_dock_backup, save_config, scan_installed_apps, sync_macos_dock, AppConfig,
    AppEntry,
};
use macos::{
    activate_app_pid, activate_this_app, configure_popover_window, dock_autohide_enabled,
    ensure_accessibility,
    frontmost_app_pid, set_dock_autohide,
    get_macos_dock_icon_rects, get_mouse_and_screen, get_running_apps, install_row_backdrop,
    perform_app_action, set_accessory_activation_policy, set_row_backdrop_frame, DockIconRect,
    ScreenPosition,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::macos::WindowExtMacOS,
    window::WindowBuilder,
};
use wry::{DragDropEvent, WebViewBuilder};

/// Folder bundles tell the running app which folder was clicked through this socket. It lives
/// in the user's own config folder: in shared /tmp another account could claim the name first.
fn socket_path() -> std::path::PathBuf {
    config::config_dir().join("ipc.sock")
}
const POPUP_DEFAULT_WIDTH: f64 = 520.0;
const POPUP_HEIGHT_COMPACT: f64 = 128.0;
/// How long the cursor may sit outside both the Dock icon and the popup before it closes.
const HOVER_HIDE_GRACE: Duration = Duration::from_millis(350);
/// While a row is open, another folder icon must stay hovered this long before the row switches,
/// so moving diagonally up into the row across a neighbouring folder doesn't flip it.
const SWITCH_INTENT: Duration = Duration::from_millis(140);
/// How far the popup's bottom sits below the top of the hovered Dock tile's frame. The tile
/// frame starts above the icon, so this lands the row right on the Dock's glass and fully
/// covers the Dock's own name label, which is drawn just above the icon.
const ROW_DROP: f64 = 4.0;

#[derive(Debug, Clone)]
enum UserEvent {
    TriggerPopover {
        folder_id: Option<String>,
        screen_pos: ScreenPosition,
        /// Top edge of the hovered native Dock icon (top-left global coords), when known.
        anchor_top: Option<f64>,
        /// Width of the hovered native Dock icon, so the popup matches the Dock's size.
        tile: Option<f64>,
        is_hover: bool,
    },
    SetLayout {
        expanded: bool,
        width: f64,
        height: f64,
        row: RowRect,
        panel: Option<RowRect>,
    },
    /// Cursor moved into the popup: take focus so clicks and hover effects work.
    BeginInteraction,
    /// Cursor left the Dock icon and the popup: close and give focus back.
    AutoHide,
    HideWindow,
    /// `.app` bundles dragged in from Finder were dropped on the row.
    AppsDropped(Vec<std::path::PathBuf>),
    /// An app drag from Finder is over the row (true) or has left it (false).
    DropHint(bool),
}

/// Where the app row sits inside the popup: horizontal offset of its left edge from the
/// window centre, distance from the window bottom, size and corner radius.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct RowRect {
    dx: f64,
    bottom: f64,
    w: f64,
    h: f64,
    radius: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct IpcTriggerMessage {
    folder_id: Option<String>,
    screen_pos: ScreenPosition,
}

struct SharedState {
    config: AppConfig,
    icon_cache: HashMap<String, Vec<u8>>,
    installed_apps_cache: Vec<config::AppEntry>,
    last_shown_at: Instant,
    last_folder_id: String,
    modal_expanded: bool,
    current_screen_pos: ScreenPosition,
    anchor_x: f64,
    anchor_top: Option<f64>,
    content_width: f64,
    content_height: f64,
    row: RowRect,
    /// Open picker/editor/menu above the row, which gets its own glass
    panel: Option<RowRect>,
    tile_size: f64,
    popup_visible: bool,
    /// (x, y, width, height) of the popup in top-left global coords.
    popup_rect: (f64, f64, f64, f64),
    interacting: bool,
    prev_front_pid: i32,
    /// Set while an app is being dragged out of the row: keeps it open and on the same folder.
    dragging_out: Option<Instant>,
    /// Folder contents changed while "Keep Dock clean" is on: sync the Dock when the row closes.
    dock_dirty: bool,
    /// Apps that left every folder since the last sync, to pin back to the Dock.
    repin: Vec<String>,
}

fn foldered_apps(cfg: &AppConfig) -> std::collections::HashSet<String> {
    cfg.folders
        .iter()
        .flat_map(|f| f.apps.iter().map(|a| a.path.trim_end_matches('/').to_string()))
        .collect()
}

/// Records a change in folder membership so the Dock follows it once the row closes.
/// Rebuilds folder bundles off the state lock: it codesigns each one and takes seconds,
/// and holding the lock that long would freeze hover and the row.
fn rebuild_bundles_in_background(state: &Arc<Mutex<SharedState>>) {
    let state = Arc::clone(state);
    std::thread::spawn(move || {
        let (cfg, mut cache) = {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            (st.config.clone(), std::mem::take(&mut st.icon_cache))
        };
        let _ = build_folder_app_bundles(&cfg, &mut cache);
        state.lock().unwrap_or_else(|e| e.into_inner()).icon_cache.extend(cache);
    });
}

/// Each folder's bundle name, which is what the Dock tile points at.
fn bundle_names(cfg: &AppConfig) -> Vec<String> {
    cfg.folders.iter().map(|f| config::sanitize_bundle_name(&f.name)).collect()
}

fn note_folder_change(st: &mut SharedState, before: &std::collections::HashSet<String>) {
    if !st.config.keep_dock_clean {
        return;
    }
    let after = foldered_apps(&st.config);
    if &after == before {
        return;
    }
    st.repin.retain(|p| !after.contains(p));
    for p in before.difference(&after) {
        if !st.repin.contains(p) {
            st.repin.push(p.clone());
        }
    }
    st.dock_dirty = true;
}

/// Longest a drag-out may hold the row open, in case the webview never reports the drag ended.
const DRAG_OUT_TIMEOUT: Duration = Duration::from_secs(30);

fn app_bundle_paths(paths: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "app") && p.is_dir())
        .cloned()
        .collect()
}

fn detect_launched_folder_id() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--folder" && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents_dir) = macos_dir.parent() {
                let fid_path = contents_dir.join("Resources").join("folder_id.txt");
                if let Ok(fid) = fs::read_to_string(fid_path) {
                    let trimmed = fid.trim().to_string();
                    if !trimmed.is_empty() {
                        return Some(trimmed);
                    }
                }
            }
        }
    }
    None
}

fn try_notify_existing_instance(folder_id: Option<String>, screen_pos: ScreenPosition) -> bool {
    if let Ok(mut stream) = UnixStream::connect(socket_path()) {
        let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
        let msg = IpcTriggerMessage {
            folder_id,
            screen_pos,
        };
        if let Ok(bytes) = serde_json::to_vec(&msg) {
            if stream.write_all(&bytes).is_ok() {
                return true;
            }
        }
    }
    false
}

fn start_unix_ipc_listener(proxy: EventLoopProxy<UserEvent>) {
    let _ = fs::remove_file(socket_path());
    if let Ok(listener) = UnixListener::bind(socket_path()) {
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut buf = Vec::new();
                let mut s = stream;
                let _ = s.set_read_timeout(Some(Duration::from_millis(300)));
                if s.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                    if let Ok(msg) = serde_json::from_slice::<IpcTriggerMessage>(&buf) {
                        // A clicked folder opens over its own Dock tile, sized like the Dock
                        let mut screen_pos = msg.screen_pos;
                        let tile_rect = msg.folder_id.as_deref().and_then(folder_dock_tile);
                        if let Some(r) = &tile_rect {
                            screen_pos.mouse_x = r.x + r.width * 0.5;
                        }
                        let _ = proxy.send_event(UserEvent::TriggerPopover {
                            folder_id: msg.folder_id,
                            screen_pos,
                            anchor_top: tile_rect.as_ref().map(|r| r.y),
                            tile: tile_rect.as_ref().map(|r| r.width),
                            is_hover: false,
                        });
                    }
                }
            }
        });
    }
}

/// The Dock tile pinned for a folder, found by its bundle path.
fn folder_dock_tile(folder_id: &str) -> Option<macos::DockIconRect> {
    let cfg = load_or_init_config();
    let name = cfg.folders.iter().find(|f| f.id == folder_id)?.name.clone();
    let stem = config::sanitize_bundle_name(&name);
    macos::get_macos_dock_icon_rects().into_iter().find(|r| {
        r.path.contains("/OpenFolders/")
            && std::path::Path::new(&r.path).file_stem().map(|s| s.to_string_lossy() == stem.as_str())
                == Some(true)
    })
}

/// Watches for mouse hover over any Folder icon in the native macOS Dock.
/// Hovering a folder pops its apps up in a horizontal row right above that icon;
/// moving off both the icon and the popup closes it again.
/// The Dock names a folder tile again whenever its bundle changes, and that name shows
/// under the row. Watch the Dock's settings and take the name off again, while no row is open.
fn start_dock_label_watcher(state: Arc<Mutex<SharedState>>) {
    std::thread::spawn(move || {
        let plist = dirs::home_dir()
            .unwrap_or_default()
            .join("Library/Preferences/com.apple.dock.plist");
        let mut seen = None;
        let mut fixes: Vec<Instant> = Vec::new();
        // Fixes in a row with no settled read between them; if the Dock keeps undoing
        // the label, stop restarting it until a later read finds the labels intact.
        let mut streak = 0;
        loop {
            std::thread::sleep(Duration::from_secs(3));
            let mtime = fs::metadata(&plist).and_then(|m| m.modified()).ok();
            if mtime == seen || state.lock().unwrap_or_else(|e| e.into_inner()).popup_visible {
                continue;
            }
            seen = mtime;
            fixes.retain(|t| t.elapsed() < Duration::from_secs(60));
            if streak >= 5 {
                if !config::folder_tile_labels_need_fix() {
                    streak = 0;
                }
                continue;
            }
            if fixes.len() < 3 {
                if config::hide_folder_tile_labels() {
                    fixes.push(Instant::now());
                    streak += 1;
                } else {
                    streak = 0;
                }
            }
        }
    });
}

fn start_dock_hover_watcher(state: Arc<Mutex<SharedState>>, proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        let mut cached_rects: Vec<DockIconRect> = Vec::new();
        let mut last_rect_fetch = Instant::now() - Duration::from_secs(10);
        let mut last_hovered_folder: Option<String> = None;
        let mut outside_since: Option<Instant> = None;
        let mut pending_switch: Option<(String, Instant)> = None;
        // Where the open row is anchored, to follow its icon if the Dock moves
        let mut last_anchor: (f64, f64) = (0.0, 0.0);

        loop {
            std::thread::sleep(Duration::from_millis(60));
            let pos = get_mouse_and_screen();

            let (visible, (rx, ry, rw, rh), expanded, interacting, folders, dragging) = {
                let st = state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    st.popup_visible,
                    st.popup_rect,
                    st.modal_expanded,
                    st.interacting,
                    st.config.folders.clone(),
                    st.dragging_out
                        .is_some_and(|t| t.elapsed() < DRAG_OUT_TIMEOUT),
                )
            };
            // Mid drag-out: keep the row open and on its folder until the drop
            if dragging {
                outside_since = None;
                continue;
            }

            // Only inspect Dock icons when mouse is near the bottom of the screen
            let mut matched_folder: Option<(String, f64, f64, f64)> = None;
            if pos.mouse_y_top_left >= pos.screen_height - 140.0 {
                // Dock icons move: auto-hide slides them in, Dock restarts re-lay them out,
                // magnification grows them. Re-read often while the cursor is down here
                // (the Accessibility read takes well under a millisecond).
                if last_rect_fetch.elapsed() > Duration::from_millis(300) || cached_rects.is_empty() {
                    cached_rects = get_macos_dock_icon_rects();
                    last_rect_fetch = Instant::now();
                }
                for rect in &cached_rects {
                    if pos.mouse_x >= rect.x
                        && pos.mouse_x <= rect.x + rect.width
                        && pos.mouse_y_top_left >= rect.y - 4.0
                        && pos.mouse_y_top_left <= rect.y + rect.height + 12.0
                    {
                        // Folder tiles are pinned without a label, so match by bundle path;
                        // titles still work for tiles pinned by older versions
                        let bundle = if rect.path.contains("/OpenFolders/") {
                            std::path::Path::new(&rect.path)
                                .file_stem()
                                .map(|s| s.to_string_lossy().to_string())
                        } else {
                            None
                        };
                        let fid = if let Some(f) = folders.iter().find(|f| {
                            bundle.as_deref() == Some(config::sanitize_bundle_name(&f.name).as_str())
                                || f.name.eq_ignore_ascii_case(&rect.title)
                        }) {
                            Some(f.id.clone())
                        } else if bundle.as_deref() == Some("OpenFolders")
                            || rect.title.eq_ignore_ascii_case("OpenFolders")
                        {
                            folders.first().map(|f| f.id.clone())
                        } else {
                            None
                        };
                        if let Some(fid) = fid {
                            matched_folder =
                                Some((fid, rect.x + rect.width * 0.5, rect.y, rect.width));
                        }
                        break;
                    }
                }
            }

            // Dock icons win over the popup, so sliding along the Dock always switches folders
            let in_popup = visible
                && pos.mouse_x >= rx - 6.0
                && pos.mouse_x <= rx + rw + 6.0
                && pos.mouse_y_top_left >= ry - 6.0
                && pos.mouse_y_top_left <= ry + rh + 2.0;

            if matched_folder.is_none() && in_popup {
                outside_since = None;
                if !interacting {
                    let _ = proxy.send_event(UserEvent::BeginInteraction);
                }
            } else if let Some((fid, center_x, top_y, tile)) = matched_folder {
                outside_since = None;
                let open_folder = state.lock().unwrap_or_else(|e| e.into_inner()).last_folder_id.clone();
                let intent_met = if !visible || fid == open_folder {
                    pending_switch = None;
                    true
                } else {
                    match &pending_switch {
                        Some((p, since)) if *p == fid => since.elapsed() >= SWITCH_INTENT,
                        _ => {
                            pending_switch = Some((fid.clone(), Instant::now()));
                            false
                        }
                    }
                };
                let anchor_moved = visible
                    && fid == open_folder
                    && ((last_anchor.0 - center_x).abs() > 1.0 || (last_anchor.1 - top_y).abs() > 1.0);
                if intent_met
                    && (last_hovered_folder.as_deref() != Some(&fid) || !visible || anchor_moved)
                {
                    pending_switch = None;
                    last_anchor = (center_x, top_y);
                    last_hovered_folder = Some(fid.clone());
                    let mut hover_pos = pos.clone();
                    hover_pos.mouse_x = center_x;
                    let _ = proxy.send_event(UserEvent::TriggerPopover {
                        folder_id: Some(fid),
                        screen_pos: hover_pos,
                        anchor_top: Some(top_y),
                        tile: Some(tile),
                        is_hover: true,
                    });
                }
            } else {
                last_hovered_folder = None;
                if visible && !expanded {
                    let since = *outside_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= HOVER_HIDE_GRACE {
                        outside_since = None;
                        let _ = proxy.send_event(UserEvent::AutoHide);
                    }
                } else {
                    outside_since = None;
                }
            }
        }
    });
}

fn url_decode(input: &str) -> String {
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn start_http_server(state: Arc<Mutex<SharedState>>, proxy: EventLoopProxy<UserEvent>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind local HTTP server");
    let port = listener.local_addr().unwrap().port();

    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let state = Arc::clone(&state);
            let proxy = proxy.clone();
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0u8; 65536];
                let mut total_read = 0usize;
                let mut header_end = None;

                while total_read < buf.len() {
                    match stream.read(&mut buf[total_read..]) {
                        Ok(0) => break,
                        Ok(n) => {
                            total_read += n;
                            if let Some(pos) = buf[..total_read]
                                .windows(4)
                                .position(|w| w == b"\r\n\r\n")
                            {
                                header_end = Some(pos + 4);
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }

                if total_read == 0 {
                    return;
                }

                let hdr_end = header_end.unwrap_or(total_read);
                let header_str = String::from_utf8_lossy(&buf[..hdr_end]).to_string();
                let first_line = header_str.lines().next().unwrap_or("");
                let parts: Vec<&str> = first_line.split_whitespace().collect();
                if parts.len() < 2 {
                    return;
                }
                let method = parts[0];
                let full_path = parts[1];

                let mut content_len = 0usize;
                for line in header_str.lines() {
                    if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_len = rest.trim().parse::<usize>().unwrap_or(0);
                    }
                }

                // Only OpenFolders' own page knows the token. Without it any web page in a
                // browser could find this port and launch or quit apps. Checked before reading
                // a body, and bodies are capped, so a stranger can't make us allocate.
                if full_path.starts_with("/api/") && !request_has_token(&header_str, full_path) {
                    send_response(&mut stream, "403 Forbidden", "text/plain", b"Forbidden", "");
                    return;
                }
                if content_len > 4 * 1024 * 1024 {
                    send_response(&mut stream, "413 Payload Too Large", "text/plain", b"Too large", "");
                    return;
                }
                let mut body_bytes = buf[hdr_end..total_read].to_vec();
                while body_bytes.len() < content_len {
                    let mut more = vec![0u8; content_len - body_bytes.len()];
                    match stream.read(&mut more) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => body_bytes.extend_from_slice(&more[..n]),
                    }
                }

                handle_http_request(
                    &mut stream,
                    method,
                    full_path,
                    &body_bytes,
                    &state,
                    &proxy,
                );
            });
        }
    });

    port
}

fn send_response(
    stream: &mut std::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    extra_headers: &str,
) {
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
        status,
        content_type,
        body.len(),
        extra_headers
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

/// Secret shared with the UI page through its URL; also saved (owner-only) so the self-test
/// can open the UI. A new one each launch.
fn ui_token() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| {
        let mut bytes = [0u8; 16];
        if let Ok(mut f) = fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut bytes);
        }
        let token: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        let path = config::config_dir().join("ui-token");
        if fs::write(&path, &token).is_ok() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        token
    })
}

/// The token arrives as an X-OpenFolders-Token header, or as `t=` in the query for images.
fn request_has_token(headers: &str, full_path: &str) -> bool {
    let token = ui_token();
    let in_header = headers.lines().any(|l| {
        l.split_once(':').is_some_and(|(k, v)| {
            k.trim().eq_ignore_ascii_case("x-openfolders-token") && v.trim() == token
        })
    });
    let in_query = full_path
        .split_once('?')
        .is_some_and(|(_, q)| q.split('&').any(|kv| kv == format!("t={}", token)));
    in_header || in_query
}

fn handle_http_request(
    stream: &mut std::net::TcpStream,
    method: &str,
    full_path: &str,
    body: &[u8],
    state: &Arc<Mutex<SharedState>>,
    proxy: &EventLoopProxy<UserEvent>,
) {
    let (path, query) = match full_path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (full_path, ""),
    };

    match (method, path) {
        ("GET", "/") => {
            let html = include_str!("ui.html");
            send_response(
                stream,
                "200 OK",
                "text/html; charset=utf-8",
                html.as_bytes(),
                "",
            );
        }
        ("GET", "/api/state") => {
            let running = get_running_apps();
            let cfg = {
                let st = state.lock().unwrap_or_else(|e| e.into_inner());
                st.config.clone()
            };
            let json = serde_json::json!({
                "config": cfg,
                "running": running,
            });
            let bytes = serde_json::to_vec(&json).unwrap_or_default();
            send_response(stream, "200 OK", "application/json", &bytes, "");
        }
        ("GET", "/api/installed-apps") => {
            let apps = {
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                if st.installed_apps_cache.is_empty() {
                    st.installed_apps_cache = scan_installed_apps();
                }
                st.installed_apps_cache.clone()
            };
            let json = serde_json::json!({ "apps": apps });
            let bytes = serde_json::to_vec(&json).unwrap_or_default();
            send_response(stream, "200 OK", "application/json", &bytes, "");
        }
        ("GET", "/api/icon") => {
            let mut app_path = String::new();
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == "path" {
                        app_path = url_decode(v);
                    }
                }
            }
            let png_opt = {
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                get_cached_app_icon(&app_path, &mut st.icon_cache)
            };
            if let Some(png) = png_opt {
                send_response(
                    stream,
                    "200 OK",
                    "image/png",
                    &png,
                    "Cache-Control: public, max-age=3600\r\n",
                );
            } else {
                send_response(stream, "404 Not Found", "text/plain", b"Not found", "");
            }
        }
        ("POST", "/api/save-config") => {
            if let Ok(new_cfg) = serde_json::from_slice::<AppConfig>(body) {
                {
                    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                    let before = foldered_apps(&st.config);
                    let names_before = bundle_names(&st.config);
                    st.config = new_cfg.clone();
                    note_folder_change(&mut st, &before);
                    // A renamed, added or deleted folder is a different bundle: re-pin when the row closes
                    if bundle_names(&st.config) != names_before {
                        st.dock_dirty = true;
                    }
                    let _ = save_config(&st.config);
                }
                rebuild_bundles_in_background(&state);
                send_response(stream, "200 OK", "application/json", br#"{"ok":true}"#, "");
            } else {
                send_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    br#"{"ok":false}"#,
                    "",
                );
            }
        }
        ("POST", "/api/set-layout") => {
            #[derive(Deserialize)]
            struct LayoutReq {
                open: bool,
                width: f64,
                height: f64,
                row: RowRect,
                #[serde(default)]
                panel: Option<RowRect>,
                #[serde(default)]
                seq: u64,
            }
            // Requests can overtake each other; only the newest layout counts
            static LAST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if let Ok(req) = serde_json::from_slice::<LayoutReq>(body)
                .map_err(|_| ())
                .and_then(|r| {
                    let prev = LAST_SEQ.fetch_max(r.seq, std::sync::atomic::Ordering::SeqCst);
                    if r.seq != 0 && r.seq <= prev { Err(()) } else { Ok(r) }
                })
            {
                let _ = proxy.send_event(UserEvent::SetLayout {
                    expanded: req.open,
                    width: req.width,
                    height: req.height,
                    row: req.row,
                    panel: req.panel,
                });
            }
            send_response(stream, "200 OK", "application/json", br#"{"ok":true}"#, "");
        }
        ("POST", "/api/app-action") => {
            #[derive(Deserialize)]
            struct ActionReq {
                path: String,
                action: String,
                #[serde(default)]
                hide_window: bool,
            }
            if let Ok(req) = serde_json::from_slice::<ActionReq>(body) {
                let res = perform_app_action(&req.path, &req.action);
                if req.hide_window {
                    let _ = proxy.send_event(UserEvent::HideWindow);
                }
                let json = match res {
                    Ok(msg) => serde_json::json!({ "ok": true, "message": msg }),
                    Err(err) => serde_json::json!({ "ok": false, "message": err }),
                };
                let bytes = serde_json::to_vec(&json).unwrap_or_default();
                send_response(stream, "200 OK", "application/json", &bytes, "");
            } else {
                send_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    br#"{"ok":false}"#,
                    "",
                );
            }
        }
        ("POST", "/api/drag") => {
            #[derive(Deserialize)]
            struct DragReq {
                active: bool,
                #[serde(default)]
                folder_id: String,
                #[serde(default)]
                path: String,
                /// The webview accepted the drop itself (reordered within the row)
                #[serde(default)]
                handled: bool,
            }
            let Ok(req) = serde_json::from_slice::<DragReq>(body) else {
                send_response(stream, "400 Bad Request", "application/json", br#"{"ok":false}"#, "");
                return;
            };
            let mut removed = false;
            let mut pinned = false;
            {
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                if req.active {
                    st.dragging_out = Some(Instant::now());
                } else {
                    st.dragging_out = None;
                    // Released outside the row: take the app out of the folder, like iOS
                    let pos = get_mouse_and_screen();
                    let (rx, ry, rw, rh) = st.popup_rect;
                    let outside = pos.mouse_x < rx
                        || pos.mouse_x > rx + rw
                        || pos.mouse_y_top_left < ry
                        || pos.mouse_y_top_left > ry + rh;
                    if !req.handled && outside && !req.path.is_empty() {
                        let before = foldered_apps(&st.config);
                        if let Some(f) = st.config.folders.iter_mut().find(|f| f.id == req.folder_id) {
                            let before = f.apps.len();
                            f.apps.retain(|a| a.path != req.path);
                            removed = f.apps.len() != before;
                        }
                        if removed {
                            let _ = save_config(&st.config);
                            note_folder_change(&mut st, &before);
                            // Dropped on the Dock: pin it there so it isn't lost (with
                            // "Keep Dock clean" on, the next sync pins it back anyway)
                            let dock_band = pos.screen_height - st.tile_size - 30.0;
                            if st.config.keep_dock_clean {
                                pinned = true;
                            } else if pos.mouse_y_top_left >= dock_band {
                                pinned = true;
                                let path = req.path.clone();
                                std::thread::spawn(move || {
                                    let _ = pin_app_to_dock(&path);
                                });
                            }
                        }
                    }
                }
            }
            if removed {
                rebuild_bundles_in_background(&state);
            }
            let json = serde_json::json!({ "ok": true, "removed": removed, "pinned": pinned });
            let bytes = serde_json::to_vec(&json).unwrap_or_default();
            send_response(stream, "200 OK", "application/json", &bytes, "");
        }
        ("POST", "/api/hide-window") => {
            let _ = proxy.send_event(UserEvent::HideWindow);
            send_response(stream, "200 OK", "application/json", br#"{"ok":true}"#, "");
        }
        ("POST", "/api/dock-sync") => {
            #[derive(Deserialize)]
            struct SyncReq {
                #[serde(default)]
                unpin_grouped_apps: bool,
            }
            let unpin = serde_json::from_slice::<SyncReq>(body)
                .map(|r| r.unpin_grouped_apps)
                .unwrap_or(false);
            // Apps just taken out of a folder still go back to the Dock
            let (cfg, repin) = {
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                st.dock_dirty = false;
                (st.config.clone(), std::mem::take(&mut st.repin))
            };
            let res = sync_macos_dock(&cfg, unpin, &repin, &mut HashMap::new());
            let json = match res {
                Ok(msg) => serde_json::json!({ "ok": true, "message": msg }),
                Err(err) => serde_json::json!({ "ok": false, "message": err }),
            };
            let bytes = serde_json::to_vec(&json).unwrap_or_default();
            send_response(stream, "200 OK", "application/json", &bytes, "");
        }
        ("POST", "/api/dock-restore") => {
            let res = restore_macos_dock_backup();
            let json = match res {
                Ok(msg) => serde_json::json!({ "ok": true, "message": msg }),
                Err(err) => serde_json::json!({ "ok": false, "message": err }),
            };
            let bytes = serde_json::to_vec(&json).unwrap_or_default();
            send_response(stream, "200 OK", "application/json", &bytes, "");
        }
        _ => {
            send_response(stream, "404 Not Found", "text/plain", b"Not Found", "");
        }
    }
}

/// Popup frame (x, y, width, height), centred on the hovered Dock icon and sitting just above it.
fn compute_popup_frame(st: &SharedState) -> (f64, f64, f64, f64) {
    let pos = &st.current_screen_pos;
    // The page measures what it shows, panels included; stay clear of the menu bar
    let h = st.content_height.min(pos.screen_height - 40.0);
    let w = st.content_width.min(pos.screen_width - 24.0);
    let min_x = pos.screen_x + 12.0;
    let max_x = (pos.screen_x + pos.screen_width - w - 12.0).max(min_x);
    let x = (st.anchor_x - w * 0.5).clamp(min_x, max_x);
    // Sit just above the Dock's glass. An auto-hidden Dock reports its icons off-screen,
    // so fall back to where a revealed Dock of this tile size would start.
    let bottom = match st.anchor_top {
        Some(t) if t < pos.screen_height - 8.0 => t + ROW_DROP,
        // Revealed Dock tiles start about tile + 22 pt above the screen bottom
        _ => pos.screen_height - st.tile_size - 22.0 + ROW_DROP,
    };
    let y = (bottom - h).max(24.0);
    (x, y, w, h)
}

fn place_window(window: &tao::window::Window, st: &mut SharedState, backdrop: (usize, usize)) {
    let (x, y, w, h) = compute_popup_frame(st);
    window.set_inner_size(LogicalSize::new(w, h));
    window.set_outer_position(LogicalPosition::new(x, y));
    st.popup_rect = (x, y, w, h);
    let r = st.row;
    set_row_backdrop_frame(backdrop.0, w * 0.5 + r.dx, r.bottom, r.w, r.h, r.radius);
    let p = st.panel.unwrap_or_default();
    set_row_backdrop_frame(backdrop.1, w * 0.5 + p.dx, p.bottom, p.w, p.h, p.radius);
}

fn hide_popup(window: &tao::window::Window, st: &mut SharedState) {
    window.set_visible(false);
    st.popup_visible = false;
    st.interacting = false;
    resume_dock_autohide();
    // Apply folder edits to the Dock now that nobody is hovering it (a Dock rewrite restarts it)
    if st.dock_dirty {
        st.dock_dirty = false;
        let cfg = st.config.clone();
        let repin = std::mem::take(&mut st.repin);
        std::thread::spawn(move || {
            let _ = sync_macos_dock(&cfg, cfg.keep_dock_clean, &repin, &mut HashMap::new());
        });
    }
}

/// Marker left while we hold an auto-hiding Dock open, so a crash can't leave auto-hide off.
fn autohide_marker() -> std::path::PathBuf {
    config::autohide_marker()
}

/// Keeps an auto-hiding Dock visible while a folder row is open, so the row behaves as part
/// of the Dock: moving up into it no longer makes the Dock slide away.
fn suspend_dock_autohide() {
    if !autohide_marker().exists() && dock_autohide_enabled() {
        let _ = fs::write(autohide_marker(), b"1");
        set_dock_autohide(false);
    }
}

extern "C" {
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
}

/// SIGTERM/SIGINT/SIGHUP skip the event loop's cleanup, so give the Dock its auto-hide back here.
extern "C" fn on_quit_signal(_sig: i32) {
    resume_dock_autohide();
    std::process::exit(0);
}

fn resume_dock_autohide() {
    if autohide_marker().exists() {
        set_dock_autohide(true);
        let _ = fs::remove_file(autohide_marker());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--self-test") {
        std::process::exit(selftest::run(&socket_path()));
    }
    if args.iter().any(|a| a == "--check-accessibility") {
        let trusted = ensure_accessibility(false);
        let rects = get_macos_dock_icon_rects().len();
        println!("accessibility: {} (Dock icons read: {})", if trusted { "on" } else { "off" }, rects);
        return;
    }
    if args.iter().any(|a| a == "--build-bundles") {
        let cfg = load_or_init_config();
        let mut cache = HashMap::new();
        if let Ok(paths) = build_folder_app_bundles(&cfg, &mut cache) {
            println!(
                "Built {} folder app bundles in ~/Applications/OpenFolders/",
                paths.len()
            );
        }
        return;
    }
    if args.iter().any(|a| a == "--sync-dock") {
        let unpin = args.iter().any(|a| a == "--collapse");
        let cfg = load_or_init_config();
        let mut cache = HashMap::new();
        match sync_macos_dock(&cfg, unpin, &[], &mut cache) {
            Ok(msg) => println!("{}", msg),
            Err(e) => eprintln!("Error syncing dock: {}", e),
        }
        return;
    }

    if args.iter().any(|a| a == "--remove-from-dock") {
        match remove_from_dock(&load_or_init_config()) {
            Ok(msg) => println!("{}", msg),
            Err(e) => eprintln!("Error updating dock: {}", e),
        }
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "OpenFolders: iPhone-style folders for the macOS Dock.\n\n\
             Usage: openfolders [option]\n\n\
             (no option)          run (hover a folder in the Dock to open it)\n\
             --sync-dock          pin your folders to the Dock\n\
             --sync-dock --collapse  pin folders and remove the apps inside them from the Dock\n\
             --build-bundles      rebuild the folder icons in ~/Applications/OpenFolders\n\
             --check-accessibility  say whether hover has the Accessibility access it needs\n\
             --self-test          check the install, permissions, Dock and row placement\n\
             --remove-from-dock   remove the folders from the Dock and put their apps back\n\
             --restore-dock       restore the Dock exactly as it was before OpenFolders"
        );
        return;
    }
    if args.iter().any(|a| a == "--restore-dock") {
        match restore_macos_dock_backup() {
            Ok(msg) => println!("{}", msg),
            Err(e) => eprintln!("Error restoring dock: {}", e),
        }
        return;
    }

    unsafe {
        // SIGHUP = 1, SIGINT = 2, SIGTERM = 15
        for sig in [1, 2, 15] {
            signal(sig, on_quit_signal);
        }
    }

    let initial_folder_id = detect_launched_folder_id();
    // A previous run may have exited while holding the Dock open
    if UnixStream::connect(socket_path()).is_err() {
        resume_dock_autohide();
    }
    let initial_screen_pos = get_mouse_and_screen();

    if try_notify_existing_instance(initial_folder_id.clone(), initial_screen_pos.clone()) {
        return;
    }

    // A folder clicked while OpenFolders isn't running: start the real app (which holds the
    // Accessibility permission) on that folder, rather than running from the folder's copy
    if let (Some(fid), Ok(exe)) = (&initial_folder_id, std::env::current_exe()) {
        let main_app = config::folders_apps_dir().join("OpenFolders.app");
        if !exe.starts_with(&main_app) && main_app.exists() {
            let _ = std::process::Command::new("/usr/bin/open")
                .arg("-a")
                .arg(&main_app)
                .args(["--args", "--folder", fid])
                .status();
            return;
        }
    }

    let mut cfg = load_or_init_config();
    // install.sh runs --build-bundles before the app's first start, and that already writes
    // config.json, so "no config file" never sees the real first run: track the pinning itself
    let first_run = !cfg.first_run_pinning_done;
    if let Some(ref fid) = initial_folder_id {
        if cfg.folders.iter().any(|f| &f.id == fid) {
            cfg.active_folder_id = fid.clone();
        }
    }

    let show_on_launch = initial_folder_id.is_some();
    let shared = Arc::new(Mutex::new(SharedState {
        last_folder_id: cfg.active_folder_id.clone(),
        config: cfg,
        icon_cache: HashMap::new(),
        installed_apps_cache: Vec::new(),
        last_shown_at: Instant::now(),
        modal_expanded: false,
        current_screen_pos: initial_screen_pos.clone(),
        anchor_x: initial_screen_pos.mouse_x,
        anchor_top: None,
        content_width: POPUP_DEFAULT_WIDTH,
        content_height: POPUP_HEIGHT_COMPACT,
        row: RowRect::default(),
        panel: None,
        tile_size: macos::dock_tile_size().map_or(48.0, |s| s + 2.0),
        popup_visible: show_on_launch,
        popup_rect: (0.0, 0.0, 0.0, 0.0),
        interacting: show_on_launch,
        prev_front_pid: 0,
        dragging_out: None,
        dock_dirty: false,
        repin: Vec::new(),
    }));

    {
        rebuild_bundles_in_background(&shared);
    }

    // Hover needs Accessibility access to read where the Dock's icons are
    ensure_accessibility(true);

    // First run: put the starter folders in the Dock so there is something to hover
    if first_run {
        let shared_bg = Arc::clone(&shared);
        std::thread::spawn(move || {
            let cfg = shared_bg.lock().unwrap_or_else(|e| e.into_inner()).config.clone();
            match sync_macos_dock(&cfg, false, &[], &mut HashMap::new()) {
                Ok(_) => {
                    let mut st = shared_bg.lock().unwrap_or_else(|e| e.into_inner());
                    st.config.first_run_pinning_done = true;
                    let _ = save_config(&st.config);
                }
                // Logged, not swallowed: launchd has nowhere else to show it
                Err(e) => eprintln!("openfolders: first-run Dock sync failed: {}", e),
            }
        });
    }

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    // Stay out of the Dock. tao would otherwise make this a regular app when its loop starts,
    // which adds an OpenFolders icon to the Dock and shifts every folder under its row.
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
        event_loop.set_activate_ignoring_other_apps(false);
    }
    let proxy = event_loop.create_proxy();

    start_unix_ipc_listener(proxy.clone());
    start_dock_label_watcher(Arc::clone(&shared));
    start_dock_hover_watcher(Arc::clone(&shared), proxy.clone());
    let port = start_http_server(Arc::clone(&shared), proxy.clone());

    set_accessory_activation_policy();

    let (ox, oy, ow, oh) = {
        let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
        let frame = compute_popup_frame(&st);
        st.popup_rect = frame;
        frame
    };
    let window = WindowBuilder::new()
        .with_title("OpenFolders")
        .with_inner_size(LogicalSize::new(ow, oh))
        .with_position(LogicalPosition::new(ox, oy))
        .with_visible(show_on_launch)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top(true)
        .with_resizable(false)
        .build(&event_loop)
        .expect("Failed to create popover window");

    configure_popover_window(window.ns_window());
    if show_on_launch {
        activate_this_app();
        window.set_focus();
    }

    let url = format!("http://127.0.0.1:{}/?t={}", port, ui_token());
    // Native file drops: dragging .app bundles from Finder onto the row adds them to the folder.
    // Any file drop is swallowed so the webview never navigates to a dropped file; drags that
    // start inside the row (reordering) carry no paths and stay with the page.
    let drop_proxy = proxy.clone();
    let drag_has_apps = std::rc::Rc::new(std::cell::Cell::new(false));
    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_drag_drop_handler(move |event| match event {
            DragDropEvent::Enter { paths, .. } => {
                let has_apps = !app_bundle_paths(&paths).is_empty();
                drag_has_apps.set(has_apps);
                if has_apps {
                    let _ = drop_proxy.send_event(UserEvent::DropHint(true));
                }
                !paths.is_empty()
            }
            DragDropEvent::Over { .. } => drag_has_apps.get(),
            DragDropEvent::Drop { paths, .. } => {
                drag_has_apps.set(false);
                let _ = drop_proxy.send_event(UserEvent::DropHint(false));
                let apps = app_bundle_paths(&paths);
                if !apps.is_empty() {
                    let _ = drop_proxy.send_event(UserEvent::AppsDropped(apps));
                }
                !paths.is_empty()
            }
            DragDropEvent::Leave => {
                if drag_has_apps.replace(false) {
                    let _ = drop_proxy.send_event(UserEvent::DropHint(false));
                }
                false
            }
            _ => false,
        })
        .with_url(&url)
        .build(&window)
        .expect("Failed to build WebView");
    let backdrop = (
        install_row_backdrop(window.ns_window(), true),
        install_row_backdrop(window.ns_window(), false),
    );

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(UserEvent::TriggerPopover {
                folder_id,
                screen_pos,
                anchor_top,
                tile,
                is_hover,
            }) => {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                let target_fid = folder_id.unwrap_or_else(|| st.config.active_folder_id.clone());
                let same_folder = st.last_folder_id == target_fid;
                let elapsed = st.last_shown_at.elapsed();

                // Clicking the folder icon in the Dock again closes the popup
                if !is_hover && st.popup_visible && same_folder && elapsed > Duration::from_millis(300)
                {
                    hide_popup(&window, &mut st);
                    return;
                }
                // Same folder already open: just follow its icon if the Dock moved it
                if is_hover && st.popup_visible && same_folder {
                    st.anchor_x = screen_pos.mouse_x;
                    st.anchor_top = anchor_top;
                    st.current_screen_pos = screen_pos;
                    place_window(&window, &mut st, backdrop);
                    return;
                }

                if st.config.folders.iter().any(|f| f.id == target_fid) {
                    st.config.active_folder_id = target_fid.clone();
                    let _ = save_config(&st.config);
                }
                st.last_folder_id = target_fid.clone();
                st.last_shown_at = Instant::now();
                st.anchor_x = screen_pos.mouse_x;
                st.anchor_top = anchor_top;
                // Size the row from the Dock's setting (a hovered tile's frame grows with
                // magnification); fall back to the tile frame. Tiles are 2 pt wider than icons.
                if let Some(t) = macos::dock_tile_size().map(|s| s + 2.0).or(tile) {
                    st.tile_size = t;
                }
                let tile_size = st.tile_size;
                st.current_screen_pos = screen_pos;
                place_window(&window, &mut st, backdrop);
                window.set_visible(true);
                suspend_dock_autohide();
                st.popup_visible = true;
                if !is_hover && !st.interacting {
                    st.interacting = true;
                    activate_this_app();
                }
                drop(st);

                let js = format!(
                    "if (window.__openFolderFromDock) window.__openFolderFromDock({}, {});",
                    serde_json::to_string(&target_fid).unwrap_or_else(|_| "\"\"".to_string()),
                    tile_size,
                );
                let _ = webview.evaluate_script(&js);
            }
            Event::UserEvent(UserEvent::SetLayout {
                expanded,
                width,
                height,
                row,
                panel,
            }) => {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                st.panel = panel;
                st.modal_expanded = expanded;
                st.content_width = width;
                st.content_height = height;
                st.row = row;
                place_window(&window, &mut st, backdrop);
            }
            Event::UserEvent(UserEvent::BeginInteraction) => {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                if !st.popup_visible || st.interacting {
                    return;
                }
                st.interacting = true;
                let front = frontmost_app_pid();
                st.prev_front_pid = if front == std::process::id() as i32 { 0 } else { front };
                activate_this_app();
                window.set_focus();
            }
            Event::UserEvent(UserEvent::AutoHide) => {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                if !st.popup_visible || st.modal_expanded {
                    return;
                }
                let restore = if st.interacting { st.prev_front_pid } else { 0 };
                hide_popup(&window, &mut st);
                st.prev_front_pid = 0;
                if restore > 0 {
                    activate_app_pid(restore);
                }
            }
            Event::UserEvent(UserEvent::HideWindow) => {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                hide_popup(&window, &mut st);
                st.prev_front_pid = 0;
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(false),
                ..
            } => {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                if st.config.hide_on_blur
                    && st.interacting
                    && !st.modal_expanded
                    && st.last_shown_at.elapsed() > Duration::from_millis(450)
                {
                    hide_popup(&window, &mut st);
                }
            }
            Event::UserEvent(UserEvent::AppsDropped(paths)) => {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                let before = foldered_apps(&st.config);
                let fid = st.last_folder_id.clone();
                let mut added: Vec<String> = Vec::new();
                if let Some(f) = st.config.folders.iter_mut().find(|f| f.id == fid) {
                    for p in paths {
                        let path = p.to_string_lossy().trim_end_matches('/').to_string();
                        if f.apps.iter().any(|a| a.path.trim_end_matches('/') == path) {
                            continue;
                        }
                        let name = p
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());
                        added.push(name.clone());
                        f.apps.push(AppEntry { name, path });
                    }
                }
                if added.is_empty() {
                    return;
                }
                let _ = save_config(&st.config);
                note_folder_change(&mut st, &before);
                drop(st);
                rebuild_bundles_in_background(&shared);
                let msg = if added.len() == 1 {
                    format!("Added {}", added[0])
                } else {
                    format!("Added {} apps", added.len())
                };
                let js = format!(
                    "refreshState(); showToast({});",
                    serde_json::to_string(&msg).unwrap_or_default()
                );
                let _ = webview.evaluate_script(&js);
            }
            Event::UserEvent(UserEvent::DropHint(on)) => {
                let _ = webview.evaluate_script(&format!(
                    "if (window.__dropHint) window.__dropHint({});",
                    on
                ));
            }
            Event::LoopDestroyed => resume_dock_autohide(),
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                hide_popup(&window, &mut st);
            }
            _ => {}
        }
    });
}
