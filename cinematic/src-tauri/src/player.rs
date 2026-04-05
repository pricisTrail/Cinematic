use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::database::Database;
use crate::models::{PlaybackLaunchResult, PlayerStateSnapshot, PlayerTrack};

const PLAYER_STATE_EVENT: &str = "player://state";
const PLAYBACK_SYNC_EVENT: &str = "cinematic://playback-updated";
const MAIN_WINDOW_LABEL: &str = "main";
#[cfg(target_os = "windows")]
const PLAYER_SURFACE_PADDING: u32 = 0;
const PLAYER_DOCK_HEIGHT: u32 = 72;
const MIN_RESUME_SECS: f64 = 5.0;
const WATCHED_COMPLETION_RATIO: f64 = 0.98;
const WATCHED_COMPLETION_GRACE_SECS: f64 = 15.0;
const IPC_READY_TIMEOUT: Duration = Duration::from_secs(6);
const IPC_RETRY_DELAY: Duration = Duration::from_millis(150);
const PROGRESS_PERSIST_INTERVAL: Duration = Duration::from_secs(2);

pub struct PlayerManager {
    session: Mutex<Option<Arc<PlayerSession>>>,
    #[cfg(target_os = "windows")]
    surface_bounds: Mutex<Option<PlayerSurfaceBounds>>,
}

impl PlayerManager {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            #[cfg(target_os = "windows")]
            surface_bounds: Mutex::new(None),
        }
    }

    fn get(&self) -> Option<Arc<PlayerSession>> {
        self.session.lock().ok().and_then(|session| session.clone())
    }

    fn set(&self, session: Arc<PlayerSession>) {
        if let Ok(mut current) = self.session.lock() {
            current.replace(session);
        }
    }

    fn clear_if_current(&self, session: &Arc<PlayerSession>) -> bool {
        if let Ok(mut current) = self.session.lock() {
            if current
                .as_ref()
                .is_some_and(|existing| Arc::ptr_eq(existing, session))
            {
                current.take();
                return true;
            }
        }

        false
    }

    pub fn snapshot(&self) -> PlayerStateSnapshot {
        self.get()
            .map(|session| session.snapshot())
            .unwrap_or_default()
    }

    pub fn current_video_id(&self) -> Option<String> {
        self.get().and_then(|session| {
            let shared = session.shared.lock().ok()?;
            shared.snapshot.video_id.clone()
        })
    }

    #[cfg(target_os = "windows")]
    fn set_surface_bounds(&self, bounds: PlayerSurfaceBounds) {
        if let Ok(mut current) = self.surface_bounds.lock() {
            current.replace(bounds);
        }
    }

    #[cfg(target_os = "windows")]
    fn surface_bounds(&self) -> Option<PlayerSurfaceBounds> {
        self.surface_bounds.lock().ok().and_then(|bounds| *bounds)
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct PlayerSurfaceBounds {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

struct PlayerSession {
    shared: Mutex<PlayerShared>,
    terminated: AtomicBool,
    /// Join handle for the dedicated host-window message-pump thread.
    /// Taken during cleanup to join the thread after posting WM_CLOSE.
    #[cfg(target_os = "windows")]
    host_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

struct PlayerShared {
    #[cfg(target_os = "windows")]
    host_hwnd: isize,
    #[cfg(target_os = "windows")]
    ipc_writer: Arc<Mutex<std::fs::File>>,
    #[cfg(target_os = "windows")]
    child: Arc<Mutex<std::process::Child>>,
    snapshot: PlayerStateSnapshot,
    pending_resume_secs: Option<f64>,
}

impl PlayerSession {
    fn snapshot(&self) -> PlayerStateSnapshot {
        self.shared
            .lock()
            .map(|shared| shared.snapshot.clone())
            .unwrap_or_default()
    }
}

fn should_mark_watched(time_secs: f64, length_secs: Option<f64>) -> bool {
    let Some(length_secs) = length_secs else {
        return false;
    };

    if length_secs <= 0.0 {
        return false;
    }

    let completion_threshold =
        (length_secs - WATCHED_COMPLETION_GRACE_SECS).max(length_secs * WATCHED_COMPLETION_RATIO);

    time_secs >= completion_threshold
}

fn derive_title(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Untitled video")
        .replace('_', " ")
        .replace('.', " ")
        .replace('-', " ")
}

fn emit_player_state(app: &AppHandle, snapshot: &PlayerStateSnapshot) {
    let _ = app.emit_to(MAIN_WINDOW_LABEL, PLAYER_STATE_EVENT, snapshot);
}

fn emit_playback_sync(app: &AppHandle, video_id: Option<&str>) {
    let _ = app.emit_to(
        MAIN_WINDOW_LABEL,
        PLAYBACK_SYNC_EVENT,
        json!({ "videoId": video_id }),
    );
}

#[cfg(target_os = "windows")]
fn write_ipc_command(writer: &Arc<Mutex<std::fs::File>>, command: Value) -> Result<(), String> {
    let mut file = writer.lock().map_err(|e| e.to_string())?;
    let payload = serde_json::to_string(&command).map_err(|e| e.to_string())?;
    file.write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn update_track_list(snapshot: &mut PlayerStateSnapshot, data: &Value) {
    let tracks = data.as_array().cloned().unwrap_or_default();
    let mut subtitles = Vec::new();
    let mut audio = Vec::new();

    for track in tracks {
        let Some(kind) = track.get("type").and_then(Value::as_str) else {
            continue;
        };
        let Some(id) = track.get("id").and_then(Value::as_i64) else {
            continue;
        };

        let normalized = PlayerTrack {
            id,
            kind: kind.to_string(),
            title: track
                .get("title")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            lang: track
                .get("lang")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            codec: track
                .get("codec")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            external: track
                .get("external")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            selected: track
                .get("selected")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };

        match kind {
            "sub" => subtitles.push(normalized),
            "audio" => audio.push(normalized),
            _ => {}
        }
    }

    snapshot.subtitle_tracks = subtitles;
    snapshot.audio_tracks = audio;
}

#[cfg(target_os = "windows")]
fn persist_snapshot(db: &Arc<Database>, snapshot: &PlayerStateSnapshot) {
    let Some(video_id) = snapshot.video_id.as_deref() else {
        return;
    };

    let is_finished = should_mark_watched(snapshot.position_secs, snapshot.duration_secs);
    if snapshot.position_secs >= MIN_RESUME_SECS || is_finished {
        let _ = db.apply_playback_progress(
            video_id,
            if is_finished {
                0.0
            } else {
                snapshot.position_secs
            },
            is_finished,
        );
    }
}

#[cfg(target_os = "windows")]
fn cleanup_session(
    app: &AppHandle,
    manager: &Arc<PlayerManager>,
    session: &Arc<PlayerSession>,
) {
    if session.terminated.swap(true, Ordering::SeqCst) {
        return;
    }

    let mut video_id = None;

    if let Ok(shared) = session.shared.lock() {
        video_id = shared.snapshot.video_id.clone();
        persist_snapshot(
            &app.state::<crate::commands::AppState>().db,
            &shared.snapshot,
        );
        let _ = write_ipc_command(&shared.ipc_writer, json!({ "command": ["quit"] }));
        if let Ok(mut child) = shared.child.lock() {
            let _ = child.kill();
        }
        // Post WM_CLOSE to the host window.  Our WndProc handles
        // WM_DESTROY → PostQuitMessage, which terminates the dedicated
        // message-pump thread.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(windows::Win32::Foundation::HWND(shared.host_hwnd as _)),
                windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
    }

    // Join the host-window thread so it terminates cleanly.
    if let Ok(mut guard) = session.host_thread.lock() {
        if let Some(handle) = guard.take() {
            let _ = handle.join();
        }
    }

    let was_current = manager.clear_if_current(session);
    emit_playback_sync(app, video_id.as_deref());
    if was_current {
        emit_player_state(app, &PlayerStateSnapshot::default());
    }
}

#[cfg(target_os = "windows")]
fn default_surface_bounds(window: &tauri::WebviewWindow) -> Result<PlayerSurfaceBounds, String> {
    let inner_size = window.inner_size().map_err(|e| e.to_string())?;
    let left = PLAYER_SURFACE_PADDING as i32;
    let top = PLAYER_SURFACE_PADDING as i32;
    let width = inner_size
        .width
        .saturating_sub(PLAYER_SURFACE_PADDING * 2)
        .max(1) as i32;
    let reserved_bottom = PLAYER_DOCK_HEIGHT + PLAYER_SURFACE_PADDING * 2;
    let height = inner_size
        .height
        .saturating_sub(reserved_bottom)
        .max(1) as i32;

    Ok(PlayerSurfaceBounds {
        left,
        top,
        width,
        height,
    })
}

#[cfg(target_os = "windows")]
fn resolve_surface_bounds(
    window: &tauri::WebviewWindow,
    manager: &Arc<PlayerManager>,
) -> Result<PlayerSurfaceBounds, String> {
    match manager.surface_bounds() {
        Some(bounds) if bounds.width > 0 && bounds.height > 0 => Ok(bounds),
        _ => default_surface_bounds(window),
    }
}

#[cfg(target_os = "windows")]
fn surface_bounds_to_screen(
    window: &tauri::WebviewWindow,
    bounds: PlayerSurfaceBounds,
) -> Result<PlayerSurfaceBounds, String> {
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let mut origin = windows::Win32::Foundation::POINT {
        x: bounds.left,
        y: bounds.top,
    };

    unsafe {
        if !windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut origin).as_bool() {
            return Err(windows::core::Error::from_win32().to_string());
        }
    }

    Ok(PlayerSurfaceBounds {
        left: origin.x,
        top: origin.y,
        ..bounds
    })
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn resize_child_proc(
    hwnd: windows::Win32::Foundation::HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    let bounds = &*(lparam.0 as *const PlayerSurfaceBounds);
    let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
        hwnd,
        None,
        0,
        0,
        bounds.width.max(1),
        bounds.height.max(1),
        windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
            | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
    );
    windows::core::BOOL(1)
}

#[cfg(target_os = "windows")]
fn layout_player_surfaces(app: &AppHandle, manager: &Arc<PlayerManager>) -> Result<(), String> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Ok(());
    };
    let Some(session) = manager.get() else {
        return Ok(());
    };
    let parent_hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let bounds = surface_bounds_to_screen(&window, resolve_surface_bounds(&window, manager)?)?;

    {
        let shared = session.shared.lock().map_err(|e| e.to_string())?;
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                windows::Win32::Foundation::HWND(shared.host_hwnd as _),
                Some(parent_hwnd),
                bounds.left,
                bounds.top,
                bounds.width.max(1),
                bounds.height.max(1),
                windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
            );

            // Resize the mpv child window to fill the host window exactly.
            let _ = windows::Win32::UI::WindowsAndMessaging::EnumChildWindows(
                Some(windows::Win32::Foundation::HWND(shared.host_hwnd as _)),
                Some(resize_child_proc),
                windows::Win32::Foundation::LPARAM(&bounds as *const _ as isize),
            );
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn wait_for_ipc(pipe_name: &str) -> Result<(std::fs::File, std::fs::File), String> {
    let deadline = Instant::now() + IPC_READY_TIMEOUT;

    // Open the writer connection first.
    let writer = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe_name)
        {
            Ok(file) => break file,
            Err(err) if Instant::now() < deadline => {
                let _ = err;
                std::thread::sleep(IPC_RETRY_DELAY);
            }
            Err(err) => {
                return Err(format!(
                    "Bundled mpv started but its IPC pipe never became ready: {err}"
                ))
            }
        }
    };

    // Open a second, independent connection for reading.
    let reader = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe_name)
        {
            Ok(file) => break file,
            Err(err) if Instant::now() < deadline => {
                let _ = err;
                std::thread::sleep(IPC_RETRY_DELAY);
            }
            Err(err) => {
                return Err(format!(
                    "IPC reader connection failed: {err}"
                ))
            }
        }
    };

    Ok((writer, reader))
}

#[cfg(target_os = "windows")]
fn resolve_mpv_runtime_dir(app: &AppHandle) -> PathBuf {
    let bundled = app
        .path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join("mpv"))
        .filter(|dir| dir.exists());

    bundled.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("mpv")
    })
}

#[cfg(target_os = "windows")]
fn resolve_mpv_log_path(app: &AppHandle) -> Result<PathBuf, String> {
    let log_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    Ok(log_dir.join("internal-player-mpv.log"))
}

#[cfg(target_os = "windows")]
fn build_mpv_command(
    app: &AppHandle,
    host_hwnd: isize,
    pipe_name: &str,
    video_path: &str,
    resume_secs: f64,
) -> Result<std::process::Command, String> {
    use tauri_plugin_shell::ShellExt;

    let sidecar = app.shell().sidecar("mpv").map_err(|err| {
        format!(
            "Bundled mpv sidecar was not found next to the app binary. Ensure `src-tauri/binaries/mpv-<target>.exe` is bundled. Details: {err}"
        )
    })?;

    let runtime_dir = resolve_mpv_runtime_dir(app);
    if !runtime_dir.is_dir() {
        return Err(format!(
            "Bundled mpv runtime directory was not found: {}",
            runtime_dir.display()
        ));
    }
    let log_file = resolve_mpv_log_path(app)?;
    let existing_path = std::env::var_os("PATH").unwrap_or_default();
    let combined_path = std::env::join_paths(
        std::iter::once(runtime_dir.clone()).chain(std::env::split_paths(&existing_path)),
    )
    .map_err(|e| e.to_string())?;

    let mut command: std::process::Command = sidecar.into();
    command
        .current_dir(&runtime_dir)
        .env("PATH", combined_path)
        .arg("--no-config")
        .arg("--idle=yes")
        .arg("--force-window=immediate")
        .arg("--osc=no")
        .arg("--sub-auto=all")
        .arg("--embeddedfonts=yes")
        .arg("--hwdec=auto-safe")
        .arg("--save-position-on-quit=no")
        .arg("--input-default-bindings=yes")
        .arg("--no-terminal")
        .arg("--audio-display=no")
        .arg("--background=color")
        .arg("--background-color=#000000")
        .arg(format!("--log-file={}", log_file.display()))
        .arg(format!("--input-ipc-server={pipe_name}"))
        .arg(format!("--wid={host_hwnd}"))
        .arg(video_path);

    if resume_secs >= MIN_RESUME_SECS {
        command.arg(format!("--start={resume_secs:.3}"));
    }

    Ok(command)
}

/// Custom WndProc for the video host window (VLC-inspired).
///
/// Unlike the `STATIC` class, this proc:
/// - Suppresses `WM_ERASEBKGND` so the surface is never wiped to white/black.
/// - Handles `WM_PAINT` by only validating the dirty region; the actual video
///   pixels are presented by mpv's child window via its own D3D11 swapchain.
/// - On `WM_DESTROY`, posts `WM_QUIT` so the dedicated message-pump thread
///   exits cleanly.
#[cfg(target_os = "windows")]
unsafe extern "system" fn video_host_wndproc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    match msg {
        // Suppress background erase — prevents the white flash that the
        // default handler would paint before mpv can re-present its frame.
        windows::Win32::UI::WindowsAndMessaging::WM_ERASEBKGND => {
            windows::Win32::Foundation::LRESULT(1)
        }
        // Validate the dirty region without drawing anything.  mpv's child
        // window handles its own painting via the D3D11 swapchain.
        windows::Win32::UI::WindowsAndMessaging::WM_PAINT => {
            let mut ps = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
            let _ = windows::Win32::Graphics::Gdi::BeginPaint(hwnd, &mut ps);
            let _ = windows::Win32::Graphics::Gdi::EndPaint(hwnd, &ps);
            windows::Win32::Foundation::LRESULT(0)
        }
        // Clean exit: WM_CLOSE → DestroyWindow → WM_DESTROY → PostQuitMessage
        // so the dedicated message-pump thread can terminate.
        windows::Win32::UI::WindowsAndMessaging::WM_DESTROY => {
            windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            windows::Win32::Foundation::LRESULT(0)
        }
        _ => windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Register the `CinematicVideoHost` window class on the current thread.
/// Safe to call multiple times — duplicate registration is silently ignored.
#[cfg(target_os = "windows")]
fn register_video_host_class() -> Result<(), String> {
    let wc = windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW {
        cbSize: std::mem::size_of::<windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW>() as u32,
        style: windows::Win32::UI::WindowsAndMessaging::CS_HREDRAW
            | windows::Win32::UI::WindowsAndMessaging::CS_VREDRAW,
        lpfnWndProc: Some(video_host_wndproc),
        lpszClassName: windows::core::w!("CinematicVideoHost"),
        ..Default::default()
    };

    let atom = unsafe { windows::Win32::UI::WindowsAndMessaging::RegisterClassExW(&wc) };
    if atom == 0 {
        // If the class already exists from a previous session, that's fine.
        let err = unsafe { windows::Win32::Foundation::GetLastError() };
        if err.0 != 1410 {
            // 1410 = ERROR_CLASS_ALREADY_EXISTS
            return Err(format!("Failed to register video host class: {err:?}"));
        }
    }
    Ok(())
}

/// Create the player host window on a **dedicated thread with its own Win32
/// message pump** (the VLC pattern).
///
/// The host window is created on a new thread that runs a standard
/// `GetMessageW` / `DispatchMessageW` loop.  This guarantees that `WM_PAINT`
/// and other messages are always dispatched — even after Win-Tab cloaks and
/// restores the window — which is the root cause of the paused-frame blank
/// screen.  The previous approach created the window on a Tauri/tokio thread
/// that had no message pump, so `WM_PAINT` was never processed.
///
/// Returns `(host_hwnd_raw, join_handle)`.  The join handle must be kept alive
/// until session cleanup, which posts `WM_CLOSE` to terminate the pump.
#[cfg(target_os = "windows")]
fn create_player_host_window(
    app: &AppHandle,
    manager: &Arc<PlayerManager>,
) -> Result<(isize, std::thread::JoinHandle<()>), String> {
    let window = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "The main Cinematic window is not available.".to_string())?;
    let parent_hwnd_raw = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
    let bounds = surface_bounds_to_screen(&window, resolve_surface_bounds(&window, manager)?)?;

    let (tx, rx) = std::sync::mpsc::channel::<Result<isize, String>>();

    let handle = std::thread::Builder::new()
        .name("cinematic-video-host".into())
        .spawn(move || {
            // --- everything below runs on the dedicated host thread ---
            if let Err(e) = register_video_host_class() {
                let _ = tx.send(Err(e));
                return;
            }

            let hwnd = unsafe {
                windows::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                    windows::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE
                        | windows::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW,
                    windows::core::w!("CinematicVideoHost"),
                    windows::core::w!(""),
                    windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                        windows::Win32::UI::WindowsAndMessaging::WS_POPUP.0
                            | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE.0
                            | windows::Win32::UI::WindowsAndMessaging::WS_CLIPCHILDREN.0
                            | windows::Win32::UI::WindowsAndMessaging::WS_CLIPSIBLINGS.0,
                    ),
                    bounds.left,
                    bounds.top,
                    bounds.width.max(1),
                    bounds.height.max(1),
                    // No owner — cross-thread owner assignment in
                    // CreateWindowExW silently fails on Windows.
                    // Z-ordering is handled by SetWindowPos below,
                    // which IS cross-thread safe.
                    None,
                    None,
                    None,
                    None,
                )
            };

            let hwnd = match hwnd {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };

            // Position behind the Tauri window and show.
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                    hwnd,
                    Some(windows::Win32::Foundation::HWND(parent_hwnd_raw as _)),
                    bounds.left,
                    bounds.top,
                    bounds.width.max(1),
                    bounds.height.max(1),
                    windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW
                        | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                );
            }

            // Send the HWND back to the caller.
            let _ = tx.send(Ok(hwnd.0 as isize));

            // ── VLC-style message pump ──────────────────────────────────
            // This loop keeps running until WM_QUIT is received (posted by
            // our WM_DESTROY handler when cleanup_session sends WM_CLOSE).
            // It ensures WM_PAINT and all other queued messages reach both
            // our video_host_wndproc AND mpv's child window.
            unsafe {
                let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
                while windows::Win32::UI::WindowsAndMessaging::GetMessageW(
                    &mut msg,
                    None,
                    0,
                    0,
                )
                .as_bool()
                {
                    let _ =
                        windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                    windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
                }
            }
        })
        .map_err(|e| format!("Failed to spawn video host thread: {e}"))?;

    let hwnd_raw = rx
        .recv()
        .map_err(|_| "Video host thread exited before sending HWND".to_string())?
        .map_err(|e| format!("Video host window creation failed: {e}"))?;

    Ok((hwnd_raw, handle))
}

#[cfg(target_os = "windows")]
fn spawn_reader_thread(
    app: AppHandle,
    db: Arc<Database>,
    manager: Arc<PlayerManager>,
    session: Arc<PlayerSession>,
    mut reader: std::fs::File,
) -> Result<(), String> {
    // Send observe_property commands on the reader connection so it receives the broadcasted events.
    for command in [
        json!({ "command": ["observe_property", 1, "time-pos"] }),
        json!({ "command": ["observe_property", 2, "duration"] }),
        json!({ "command": ["observe_property", 3, "pause"] }),
        json!({ "command": ["observe_property", 4, "volume"] }),
        json!({ "command": ["observe_property", 5, "track-list"] }),
        json!({ "command": ["observe_property", 6, "sid"] }),
        json!({ "command": ["observe_property", 7, "aid"] }),
        json!({ "command": ["get_property", "track-list"], "request_id": 100 }),
        json!({ "command": ["get_property", "volume"], "request_id": 101 }),
    ] {
        if let Ok(payload) = serde_json::to_string(&command) {
            let _ = reader.write_all(format!("{}\n", payload).as_bytes());
        }
    }

    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(reader);
        let mut buffer = String::new();
        let mut last_persist = Instant::now();

        loop {
            buffer.clear();
            match reader.read_line(&mut buffer) {
                Ok(0) => {
                    cleanup_session(&app, &manager, &session);
                    break;
                }
                Ok(_) => {}
                Err(_) => {
                    cleanup_session(&app, &manager, &session);
                    break;
                }
            }

            let payload: Value = match serde_json::from_str(buffer.trim()) {
                Ok(payload) => payload,
                Err(_) => continue,
            };

            let mut emit_state = false;
            let mut emit_sync = false;
            let mut should_cleanup = false;

            if let Ok(mut shared) = session.shared.lock() {
                if let Some(event_name) = payload.get("event").and_then(Value::as_str) {
                    match event_name {
                        "file-loaded" => {
                            shared.snapshot.is_loaded = true;
                            if let Some(pending_resume_secs) = shared.pending_resume_secs.take() {
                                let _ = write_ipc_command(
                                    &shared.ipc_writer,
                                    json!({ "command": ["seek", pending_resume_secs, "absolute", "exact"] }),
                                );
                            }
                            emit_state = true;
                        }
                        "end-file" => {
                            persist_snapshot(&db, &shared.snapshot);
                            emit_sync = true;
                        }
                        "shutdown" => {
                            should_cleanup = true;
                        }
                        _ => {}
                    }
                }

                if payload.get("event").and_then(Value::as_str) == Some("property-change") {
                    let property = payload.get("name").and_then(Value::as_str).unwrap_or("");
                    let data = payload.get("data").cloned().unwrap_or(Value::Null);

                    match property {
                        "time-pos" => {
                            shared.snapshot.position_secs = data.as_f64().unwrap_or(0.0).max(0.0);
                            emit_state = true;
                        }
                        "duration" => {
                            shared.snapshot.duration_secs = data.as_f64();
                            emit_state = true;
                        }
                        "pause" => {
                            shared.snapshot.paused = data.as_bool().unwrap_or(false);
                            emit_state = true;
                        }
                        "volume" => {
                            shared.snapshot.volume =
                                data.as_f64().unwrap_or(shared.snapshot.volume);
                            emit_state = true;
                        }
                        "track-list" => {
                            update_track_list(&mut shared.snapshot, &data);
                            emit_state = true;
                        }
                        "sid" => {
                            shared.snapshot.active_subtitle_id = data.as_i64();
                            emit_state = true;
                        }
                        "aid" => {
                            shared.snapshot.active_audio_id = data.as_i64();
                            emit_state = true;
                        }
                        _ => {}
                    }
                }

                if let Some(request_id) = payload.get("request_id").and_then(Value::as_i64) {
                    match request_id {
                        100 => {
                            update_track_list(
                                &mut shared.snapshot,
                                payload.get("data").unwrap_or(&Value::Null),
                            );
                            emit_state = true;
                        }
                        101 => {
                            if let Some(volume) = payload.get("data").and_then(Value::as_f64) {
                                shared.snapshot.volume = volume;
                                emit_state = true;
                            }
                        }
                        _ => {}
                    }
                }

                if last_persist.elapsed() >= PROGRESS_PERSIST_INTERVAL
                    || should_mark_watched(
                        shared.snapshot.position_secs,
                        shared.snapshot.duration_secs,
                    )
                {
                    persist_snapshot(&db, &shared.snapshot);
                    last_persist = Instant::now();
                    emit_sync = true;
                }

                if emit_state {
                    emit_player_state(&app, &shared.snapshot);
                }
            }

            if emit_sync {
                emit_playback_sync(&app, manager.current_video_id().as_deref());
            }

            if should_cleanup {
                cleanup_session(&app, &manager, &session);
                break;
            }
        }
    });

    Ok(())
}

#[cfg(target_os = "windows")]
fn create_session(
    app: &AppHandle,
    db: Arc<Database>,
    manager: Arc<PlayerManager>,
    video_id: &str,
    video_path: &str,
    resume_secs: f64,
) -> Result<Arc<PlayerSession>, String> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.set_focus();
    }
    let main_hwnd = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "The main Cinematic window is not available.".to_string())?
        .hwnd()
        .map_err(|e| e.to_string())?
        .0 as isize;

    let (host_hwnd_raw, host_thread) = create_player_host_window(app, &manager)?;

    // Helper: tear down the host window on error by posting WM_CLOSE to
    // the dedicated message-pump thread (DestroyWindow must be called
    // from the thread that created the window).
    let close_host = |host_hwnd_raw: isize| unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(windows::Win32::Foundation::HWND(host_hwnd_raw as _)),
            windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        );
    };

    let pipe_name = format!(r"\\.\pipe\cinematic-mpv-{}", uuid::Uuid::new_v4());
    let mut command = match build_mpv_command(app, host_hwnd_raw, &pipe_name, video_path, resume_secs)
    {
        Ok(command) => command,
        Err(err) => {
            close_host(host_hwnd_raw);
            return Err(err);
        }
    };
    let mut child = match command.spawn().map_err(|e| e.to_string()) {
        Ok(child) => child,
        Err(err) => {
            close_host(host_hwnd_raw);
            return Err(err);
        }
    };
    let (ipc_file, ipc_reader) = match wait_for_ipc(&pipe_name) {
        Ok(files) => files,
        Err(err) => {
            let _ = child.kill();
            close_host(host_hwnd_raw);
            return Err(err);
        }
    };
    let ipc_writer = Arc::new(Mutex::new(ipc_file));
    let child = Arc::new(Mutex::new(child));

    let title = db
        .get_video_by_id(video_id)?
        .map(|video| video.title)
        .unwrap_or_else(|| derive_title(video_path));
    let fullscreen = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "The main Cinematic window is not available.".to_string())?
        .is_fullscreen()
        .map_err(|e| e.to_string())?;

    let session = Arc::new(PlayerSession {
        shared: Mutex::new(PlayerShared {
            host_hwnd: host_hwnd_raw,
            ipc_writer: ipc_writer.clone(),
            child: child.clone(),
            snapshot: PlayerStateSnapshot {
                video_id: Some(video_id.to_string()),
                video_path: Some(video_path.to_string()),
                title: Some(title),
                position_secs: resume_secs.max(0.0),
                fullscreen,
                ..Default::default()
            },
            pending_resume_secs: None,
        }),
        terminated: AtomicBool::new(false),
        host_thread: Mutex::new(Some(host_thread)),
    });

    manager.set(session.clone());
    layout_player_surfaces(app, &manager)?;
    spawn_focus_watch_thread(app.clone(), manager.clone(), session.clone(), main_hwnd);
    spawn_reader_thread(app.clone(), db, manager.clone(), session.clone(), ipc_reader)?;
    emit_player_state(app, &session.snapshot());

    Ok(session)
}

#[cfg(target_os = "windows")]
fn session_is_running(session: &Arc<PlayerSession>) -> bool {
    let Ok(shared) = session.shared.lock() else {
        return false;
    };

    let Ok(mut child) = shared.child.lock() else {
        return false;
    };

    matches!(child.try_wait(), Ok(None))
}

#[cfg(target_os = "windows")]
fn load_video_into_session(
    app: &AppHandle,
    db: Arc<Database>,
    session: &Arc<PlayerSession>,
    video_id: &str,
    video_path: &str,
    resume_secs: f64,
) -> Result<(), String> {
    let title = db
        .get_video_by_id(video_id)?
        .map(|video| video.title)
        .unwrap_or_else(|| derive_title(video_path));

    let mut shared = session.shared.lock().map_err(|e| e.to_string())?;
    if shared.snapshot.video_id.as_deref() == Some(video_id)
        && shared.snapshot.video_path.as_deref() == Some(video_path)
    {
        return Ok(());
    }

    let previous_video_id = shared.snapshot.video_id.clone();
    persist_snapshot(&db, &shared.snapshot);

    shared.snapshot.video_id = Some(video_id.to_string());
    shared.snapshot.video_path = Some(video_path.to_string());
    shared.snapshot.title = Some(title);
    shared.snapshot.position_secs = 0.0;
    shared.snapshot.duration_secs = None;
    shared.snapshot.is_loaded = false;
    shared.snapshot.paused = false;
    shared.snapshot.subtitle_tracks.clear();
    shared.snapshot.audio_tracks.clear();
    shared.snapshot.active_subtitle_id = None;
    shared.snapshot.active_audio_id = None;
    shared.pending_resume_secs = (resume_secs >= MIN_RESUME_SECS).then_some(resume_secs);

    write_ipc_command(
        &shared.ipc_writer,
        json!({ "command": ["loadfile", video_path, "replace"] }),
    )?;
    emit_player_state(app, &shared.snapshot);
    emit_playback_sync(app, previous_video_id.as_deref());
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn open(
    app: AppHandle,
    db: Arc<Database>,
    manager: Arc<PlayerManager>,
    video_id: String,
    video_path: String,
    resume_secs: f64,
) -> Result<PlaybackLaunchResult, String> {
    let session = match manager.get() {
        Some(session) if session_is_running(&session) => session,
        Some(session) => {
            cleanup_session(&app, &manager, &session);
            create_session(
                &app,
                db.clone(),
                manager.clone(),
                &video_id,
                &video_path,
                resume_secs,
            )?
        }
        None => create_session(
            &app,
            db.clone(),
            manager.clone(),
            &video_id,
            &video_path,
            resume_secs,
        )?,
    };

    if session_is_running(&session) {
        load_video_into_session(&app, db, &session, &video_id, &video_path, resume_secs)?;
    }

    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.set_focus();
    }

    Ok(PlaybackLaunchResult {
        tracking_enabled: true,
        already_running: false,
        resumed: resume_secs >= MIN_RESUME_SECS,
        resume_position_secs: resume_secs.max(0.0),
        fallback_used: false,
        message: if resume_secs >= MIN_RESUME_SECS {
            "Opened in the Cinematic player and resumed from your last saved position.".to_string()
        } else {
            "Opened in the Cinematic player.".to_string()
        },
    })
}

#[cfg(target_os = "windows")]
pub fn get_snapshot(manager: Arc<PlayerManager>) -> PlayerStateSnapshot {
    manager.snapshot()
}

#[cfg(target_os = "windows")]
pub fn set_surface_bounds(
    app: AppHandle,
    manager: Arc<PlayerManager>,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let window = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "The main Cinematic window is not available.".to_string())?;
    let scale_factor = window.scale_factor().map_err(|e| e.to_string())?;
    let normalize = |value: f64| -> i32 { (value.max(0.0) * scale_factor).round() as i32 };
    manager.set_surface_bounds(PlayerSurfaceBounds {
        left: normalize(left),
        top: normalize(top),
        width: normalize(width).max(1),
        height: normalize(height).max(1),
    });

    layout_player_surfaces(&app, &manager)
}

#[cfg(target_os = "windows")]
pub fn toggle_pause(manager: Arc<PlayerManager>) -> Result<(), String> {
    let Some(session) = manager.get() else {
        return Err("The internal player is not open.".to_string());
    };
    let shared = session.shared.lock().map_err(|e| e.to_string())?;
    write_ipc_command(&shared.ipc_writer, json!({ "command": ["cycle", "pause"] }))
}

#[cfg(target_os = "windows")]
pub fn refresh_paused_frame(manager: Arc<PlayerManager>) -> Result<(), String> {
    let Some(session) = manager.get() else {
        return Ok(());
    };
    let (ipc_writer, paused_position_secs) = {
        let shared = session.shared.lock().map_err(|e| e.to_string())?;
        if !shared.snapshot.is_loaded || !shared.snapshot.paused {
            return Ok(());
        }

        (shared.ipc_writer.clone(), shared.snapshot.position_secs.max(0.0))
    };

    // Ask mpv to re-decode and redisplay the exact current frame.
    // Seeking to the current position with "exact" forces the video
    // output to refresh without changing the playback state at all —
    // no pause toggle, no frame drift.
    //
    // NOTE: Do NOT InvalidateRect / RedrawWindow the host window here.
    // That erases the surface (painting the host's background) before
    // mpv has a chance to redraw, producing a white flash on Win-Tab.
    // layout_player_surfaces already calls SetWindowPos(SWP_SHOWWINDOW)
    // which is sufficient for DWM visibility.
    let _ = write_ipc_command(
        &ipc_writer,
        json!({ "command": ["seek", paused_position_secs, "absolute", "exact"] }),
    );

    Ok(())
}

#[cfg(target_os = "windows")]
fn spawn_focus_watch_thread(
    app: AppHandle,
    manager: Arc<PlayerManager>,
    session: Arc<PlayerSession>,
    main_hwnd: isize,
) {
    // Read the host_hwnd once — it does not change during the session.
    let host_hwnd = session
        .shared
        .lock()
        .ok()
        .map(|s| s.host_hwnd)
        .unwrap_or(0);

    std::thread::spawn(move || {
        let mut had_focus = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize
                == main_hwnd
        };
        let mut host_was_visible = host_hwnd != 0 && unsafe {
            windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(
                windows::Win32::Foundation::HWND(host_hwnd as _),
            )
            .as_bool()
        };
        let mut refresh_pending = false;
        let mut last_refresh = Instant::now();
        const REFRESH_COOLDOWN: Duration = Duration::from_millis(300);

        while !session.terminated.load(Ordering::SeqCst) {
            let has_focus = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize
                    == main_hwnd
            };

            // Primary trigger: foreground-window transition (Alt-Tab).
            if has_focus && !had_focus {
                refresh_pending = true;
            }

            // Secondary trigger: host window visibility restored.
            // During Win-Tab, the owned popup is auto-hidden when the
            // owner (Tauri window) is cloaked by DWM.  When the owner
            // is restored the popup becomes visible again.  This
            // catches Win-Tab even if GetForegroundWindow never changes.
            if host_hwnd != 0 {
                let host_is_visible = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(
                        windows::Win32::Foundation::HWND(host_hwnd as _),
                    )
                    .as_bool()
                };
                if host_is_visible && !host_was_visible {
                    refresh_pending = true;
                }
                host_was_visible = host_is_visible;
            }

            // Only perform the actual refresh when the cooldown has elapsed.
            // This prevents rapid Alt-Tab from flooding mpv's IPC pipe with
            // seek commands, which overwhelms the player and can crash it.
            if has_focus && refresh_pending && last_refresh.elapsed() >= REFRESH_COOLDOWN {
                let _ = layout_player_surfaces(&app, &manager);
                let _ = refresh_paused_frame(manager.clone());
                last_refresh = Instant::now();
                refresh_pending = false;
            }

            had_focus = has_focus;
            std::thread::sleep(Duration::from_millis(75));
        }
    });
}

#[cfg(target_os = "windows")]
pub fn seek_relative(manager: Arc<PlayerManager>, seconds: f64) -> Result<(), String> {
    let Some(session) = manager.get() else {
        return Err("The internal player is not open.".to_string());
    };
    let shared = session.shared.lock().map_err(|e| e.to_string())?;
    write_ipc_command(
        &shared.ipc_writer,
        json!({ "command": ["seek", seconds, "relative"] }),
    )
}

#[cfg(target_os = "windows")]
pub fn seek_to(manager: Arc<PlayerManager>, seconds: f64) -> Result<(), String> {
    let Some(session) = manager.get() else {
        return Err("The internal player is not open.".to_string());
    };
    let shared = session.shared.lock().map_err(|e| e.to_string())?;
    write_ipc_command(
        &shared.ipc_writer,
        json!({ "command": ["seek", seconds.max(0.0), "absolute", "exact"] }),
    )
}

#[cfg(target_os = "windows")]
pub fn set_speed(manager: Arc<PlayerManager>, speed: f64) -> Result<(), String> {
    let Some(session) = manager.get() else {
        return Err("The internal player is not open.".to_string());
    };
    let shared = session.shared.lock().map_err(|e| e.to_string())?;
    write_ipc_command(
        &shared.ipc_writer,
        json!({ "command": ["set_property", "speed", speed.max(0.1)] }),
    )
}

#[cfg(target_os = "windows")]
pub fn set_volume(manager: Arc<PlayerManager>, volume: f64) -> Result<(), String> {
    let Some(session) = manager.get() else {
        return Err("The internal player is not open.".to_string());
    };
    let shared = session.shared.lock().map_err(|e| e.to_string())?;
    write_ipc_command(
        &shared.ipc_writer,
        json!({ "command": ["set_property", "volume", volume.clamp(0.0, 100.0)] }),
    )
}

#[cfg(target_os = "windows")]
pub fn set_subtitle_track(
    manager: Arc<PlayerManager>,
    track_id: Option<i64>,
) -> Result<(), String> {
    let Some(session) = manager.get() else {
        return Err("The internal player is not open.".to_string());
    };
    let shared = session.shared.lock().map_err(|e| e.to_string())?;
    let value = track_id
        .map(Value::from)
        .unwrap_or_else(|| Value::String("no".to_string()));
    write_ipc_command(
        &shared.ipc_writer,
        json!({ "command": ["set_property", "sid", value] }),
    )
}

#[cfg(target_os = "windows")]
pub fn set_audio_track(manager: Arc<PlayerManager>, track_id: Option<i64>) -> Result<(), String> {
    let Some(session) = manager.get() else {
        return Err("The internal player is not open.".to_string());
    };
    let shared = session.shared.lock().map_err(|e| e.to_string())?;
    let value = track_id
        .map(Value::from)
        .unwrap_or_else(|| Value::String("no".to_string()));
    write_ipc_command(
        &shared.ipc_writer,
        json!({ "command": ["set_property", "aid", value] }),
    )
}

#[cfg(target_os = "windows")]
pub fn toggle_fullscreen(app: AppHandle, manager: Arc<PlayerManager>) -> Result<(), String> {
    if manager.get().is_none() {
        return Err("The internal player is not open.".to_string());
    }
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Err("The internal player is not open.".to_string());
    };
    let next = !window.is_fullscreen().map_err(|e| e.to_string())?;
    window.set_fullscreen(next).map_err(|e| e.to_string())?;
    if let Some(session) = manager.get() {
        if let Ok(mut shared) = session.shared.lock() {
            shared.snapshot.fullscreen = next;
            emit_player_state(&app, &shared.snapshot);
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn close(app: AppHandle, manager: Arc<PlayerManager>) -> Result<(), String> {
    if let Some(session) = manager.get() {
        cleanup_session(&app, &manager, &session);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn open(
    _app: AppHandle,
    _db: Arc<Database>,
    _manager: Arc<PlayerManager>,
    _video_id: String,
    _video_path: String,
    _resume_secs: f64,
) -> Result<PlaybackLaunchResult, String> {
    Err("The bundled internal mpv player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn get_snapshot(manager: Arc<PlayerManager>) -> PlayerStateSnapshot {
    manager.snapshot()
}

#[cfg(not(target_os = "windows"))]
pub fn set_surface_bounds(
    _app: AppHandle,
    _manager: Arc<PlayerManager>,
    _left: f64,
    _top: f64,
    _width: f64,
    _height: f64,
) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn toggle_pause(_manager: Arc<PlayerManager>) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn refresh_paused_frame(_manager: Arc<PlayerManager>) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn seek_relative(_manager: Arc<PlayerManager>, _seconds: f64) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn seek_to(_manager: Arc<PlayerManager>, _seconds: f64) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn set_speed(_manager: Arc<PlayerManager>, _speed: f64) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn set_volume(_manager: Arc<PlayerManager>, _volume: f64) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn set_subtitle_track(
    _manager: Arc<PlayerManager>,
    _track_id: Option<i64>,
) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn set_audio_track(_manager: Arc<PlayerManager>, _track_id: Option<i64>) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn toggle_fullscreen(_app: AppHandle, _manager: Arc<PlayerManager>) -> Result<(), String> {
    Err("The internal player is only available on Windows right now.".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn close(_app: AppHandle, _manager: Arc<PlayerManager>) -> Result<(), String> {
    Ok(())
}


