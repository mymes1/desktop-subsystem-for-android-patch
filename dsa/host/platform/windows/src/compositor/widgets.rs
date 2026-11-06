//! The widgets of the user's Android, as borderless windows on the user's desktop.
//!
//! The window manager watches the widgets placed on the user's desktop over the local API and
//! shows each one in a window of its own ([`sync`]). A widget window is a layered popup
//! window that belongs to the desktop: it is owned by the shell's desktop window, so it stays
//! when the user shows the desktop (Win+D), it is kept below every other window, and as a
//! tool window that never takes the focus it is neither in the taskbar nor in Alt+Tab
//! ([`window_styles`]).
//!
//! The window shows the widget's latest frame, a PNG image with an alpha channel, so a widget
//! with rounded corners or a see-through background looks as it does on a phone. The window
//! is moved by dragging the band along its top edge, which shows while the pointer is over
//! it, and resized by its edges ([`hit_test`]). When the user lets go, the service is told
//! where the window is and how large, in its monitor's pixels and density; the service
//! remembers it and tells Android, which draws the widget at that size and tells the widget's
//! app its size in dp, so the app can pick the layout that fits. A press, a move and a
//! release inside the widget go to the widget's view in Android, so its buttons and lists
//! work and an app it opens shows as a window.
//!
//! The window manager starts at sign-in, and it starts Android then when it is not running
//! ([`starts_android_at_sign_in`]): the first app opened after the computer starts does not
//! wait for Android to boot, and the widgets the service remembers come back.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dsa_host_core::host_api::HostApiClient;
use dsa_host_core::session::SessionState;
use dsa_host_core::widgets::{self, Placement, Widget, WidgetEvent, WidgetFrame};
use dsa_protocol::host_api::types::PointerAction;
use dsa_transport::Connector;

use crate::compositor::log::{note, CompositorLogger};

/// What the window manager calls itself to the service.
const BUILD: &str = "dsa-wm widgets";

/// How long a request to the service is waited for.
const ANSWER: Duration = Duration::from_secs(5);

/// How long to wait before connecting again after the service could not be reached.
const RETRY: Duration = Duration::from_secs(1);

/// How long the watch waits for the service before it passes on what the windows reported.
const POLL: Duration = Duration::from_millis(15);

/// The width of the edges a widget window is resized by, in device-independent pixels.
pub const BORDER_DIP: i32 = 6;

/// The height of the band along a widget window's top edge it is moved by.
pub const CAPTION_DIP: i32 = 18;

/// The host's density at a scale of 100%.
const HOST_BASE_DPI: u32 = 96;

/// `dp` at a monitor of `dpi` (96 at a scale of 100%), in pixels, rounded to the nearest.
/// A dp is one device-independent pixel.
pub fn px_of_dip(dp: u32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 { HOST_BASE_DPI } else { dpi };
    ((u64::from(dp) * u64::from(dpi) + u64::from(HOST_BASE_DPI) / 2) / u64::from(HOST_BASE_DPI))
        .min(i32::MAX as u64) as i32
}

/// `px` pixels at a monitor of `dpi`, in dp, rounded to the nearest.
pub fn dip_of_px(px: i32, dpi: u32) -> u32 {
    let dpi = if dpi == 0 { HOST_BASE_DPI } else { dpi };
    let px = px.max(0) as u64;
    ((px * u64::from(HOST_BASE_DPI) + u64::from(dpi) / 2) / u64::from(dpi)) as u32
}

/// Android's density for a monitor of `dpi`: 160 at a scale of 100%.
pub fn android_density(dpi: u32) -> u32 {
    let dpi = if dpi == 0 { HOST_BASE_DPI } else { dpi };
    (dpi * widgets::BASE_DPI + HOST_BASE_DPI / 2) / HOST_BASE_DPI
}

/// The size of a widget's window on a monitor of `dpi`, in pixels.
pub fn window_size(placement: &Placement, dpi: u32) -> (i32, i32) {
    (
        px_of_dip(placement.width_dp, dpi).max(1),
        px_of_dip(placement.height_dp, dpi).max(1),
    )
}

/// The placement of a window at `x`, `y` of `width` by `height` pixels on a monitor of `dpi`.
pub fn placement_of(x: i32, y: i32, width: i32, height: i32, dpi: u32) -> Placement {
    Placement {
        x,
        y,
        width_dp: dip_of_px(width, dpi).max(1),
        height_dp: dip_of_px(height, dpi).max(1),
    }
}

/// The smallest a widget's window may be made on a monitor of `dpi`, in pixels.
pub fn min_window_size(provider: &widgets::WidgetProvider, dpi: u32) -> (i32, i32) {
    let (width, height) = widgets::clamp_size(provider, 0, 0);
    (px_of_dip(width, dpi), px_of_dip(height, dpi))
}

/// The window style and extended style of a widget window: a popup with no frame, layered
/// so it can be partly see-through, a tool window so it is neither in the taskbar nor in
/// Alt+Tab, and one that never takes the focus, so clicking it leaves the user's window
/// active.
pub fn window_styles() -> (u32, u32) {
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const WS_EX_LAYERED: u32 = 0x0008_0000;
    const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
    (
        WS_POPUP,
        WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
    )
}

/// How long a widget window waits for its first image from Android, after it opened, before
/// the window manager logs that it has none. Android draws a widget within a second of being
/// told its size, so a window still without an image after this shows only its
/// placeholder, and the log says so.
pub const NO_FRAME: Duration = Duration::from_secs(10);

/// How often the widget windows are checked for an image that never came.
pub const NO_FRAME_CHECK: Duration = Duration::from_secs(5);

/// Whether a window that has waited for an image since `waiting_since` (None: it has one) is
/// overdue at `now`, and not logged yet.
pub fn frame_overdue(waiting_since: Option<Instant>, noted: bool, now: Instant) -> bool {
    match waiting_since {
        Some(since) => !noted && now.saturating_duration_since(since) >= NO_FRAME,
        None => false,
    }
}

/// The corner radius of a widget window's placeholder, in device-independent pixels.
const PLACEHOLDER_RADIUS_DIP: u32 = 12;

/// What a widget window shows until its first image from Android comes, or while the image
/// it has is all clear: a dark grey card with a lighter edge and rounded corners, as
/// premultiplied BGRA rows of `width` by `height` pixels on a monitor of `dpi`. It is mostly
/// opaque, so the user sees where the widget is and can grab it even when Android has not
/// drawn it yet; a window that showed nothing would look like no widget at all.
pub fn placeholder(width: usize, height: usize, dpi: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; width * height * 4];
    let radius = (px_of_dip(PLACEHOLDER_RADIUS_DIP, dpi).max(1) as usize)
        .min(width / 2)
        .min(height / 2) as i64;
    let edge = px_of_dip(1, dpi).max(1) as i64;
    let (w, h) = (width as i64, height as i64);
    // Premultiplied BGRA: #383838 at 0xd8 inside, #909090 at 0xe8 on the edge.
    let fill = [0x2f, 0x2f, 0x2f, 0xd8];
    let border = [0x83, 0x83, 0x83, 0xe8];
    for y in 0..h {
        for x in 0..w {
            // How far into a rounded corner's square the pixel is, 0 outside the squares.
            let cx = if x < radius {
                radius - x
            } else if x >= w - radius {
                x - (w - radius - 1)
            } else {
                0
            };
            let cy = if y < radius {
                radius - y
            } else if y >= h - radius {
                y - (h - radius - 1)
            } else {
                0
            };
            let in_corner = cx > 0 && cy > 0;
            if in_corner && cx * cx + cy * cy > radius * radius {
                continue;
            }
            let inner = (radius - edge).max(0);
            let on_edge = x < edge
                || y < edge
                || x >= w - edge
                || y >= h - edge
                || (in_corner && cx * cx + cy * cy > inner * inner);
            let at = ((y * w + x) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(if on_edge { &border } else { &fill });
        }
    }
    pixels
}

/// Whether premultiplied BGRA `pixels` show nothing: every alpha is zero. A widget always
/// draws something, so such an image is a drawing that went wrong, and the window shows its
/// placeholder instead of a see-through hole.
pub fn all_clear(pixels: &[u8]) -> bool {
    pixels.chunks_exact(4).all(|pixel| pixel[3] == 0)
}

/// Where a widget window goes in the z-order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZPlace {
    /// Right above the desktop: below the window this names, which is right above it.
    After(isize),
    /// It is right above the desktop already, or the desktop is the top window; the z-order
    /// stays.
    Keep,
    /// No desktop was found: the bottom of the z-order.
    Bottom,
}

/// The z-order a widget window `own` takes: right above the desktop's own window (`desktop`,
/// the one that holds the desktop icons), with `above_desktop` the window right above that
/// now. That is below every other window, but not the bottom of the z-order: once a
/// wallpaper slideshow or "show desktop" has run, the shell moves the icons into a window of
/// its own above `Progman` that paints the wallpaper, and a window at the bottom is hidden
/// under it.
pub fn z_place(desktop: Option<isize>, above_desktop: Option<isize>, own: isize) -> ZPlace {
    match (desktop, above_desktop) {
        (None, _) => ZPlace::Bottom,
        (Some(_), Some(above)) if above == own => ZPlace::Keep,
        (Some(_), Some(above)) => ZPlace::After(above),
        (Some(_), None) => ZPlace::Keep,
    }
}

/// How often the widget windows are checked to be right above the desktop. The shell
/// restacks its own windows without telling anyone: "show desktop", a wallpaper slideshow
/// and an Explorer restart raise the window that holds the desktop icons and paints the
/// wallpaper, and a widget window that only fixed its place when it moved itself stayed under
/// it, drawn but hidden behind the desktop.
pub const RESTACK_CHECK: Duration = Duration::from_millis(500);

/// One window below a widget window, as [`right_above_desktop`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Below {
    pub hwnd: isize,
    /// Whether it shows over part of the widget window: visible, not minimized and
    /// overlapping it.
    pub covers: bool,
}

/// Whether a widget window sits where it belongs, given the windows below it in z-order from
/// the nearest down (`below`): the desktop's own window (`desktop`) comes before any other
/// window that overlaps it. Other widget windows (`ours`) and windows that do not overlap it
/// may be in between. False when the desktop is not below it at all, which means the desktop
/// is drawn over it, and when an app's window is below it, which it would hide.
pub fn right_above_desktop(below: &[Below], desktop: isize, ours: &[isize]) -> bool {
    for window in below {
        if window.hwnd == desktop {
            return true;
        }
        if window.covers && !ours.contains(&window.hwnd) {
            return false;
        }
    }
    false
}

/// Where in a widget window a point is, which says what dragging it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Inside the widget: input goes to Android.
    Client,
    /// The band along the top: dragging moves the window.
    Caption,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// What `x`, `y` (from the window's top left corner) hits in a window of `width` by `height`
/// pixels on a monitor of `dpi`. Only the edges the widget may be resized along resize it.
pub fn hit_test(x: i32, y: i32, width: i32, height: i32, dpi: u32, resize_mode: u32) -> Hit {
    let border = px_of_dip(BORDER_DIP as u32, dpi).max(1);
    let caption = px_of_dip(CAPTION_DIP as u32, dpi).max(1);
    let horizontal = resize_mode & widgets::RESIZE_HORIZONTAL != 0;
    let vertical = resize_mode & widgets::RESIZE_VERTICAL != 0;
    let left = horizontal && x < border;
    let right = horizontal && x >= width - border;
    let top = vertical && y < border;
    let bottom = vertical && y >= height - border;
    match (left, right, top, bottom) {
        (true, _, true, _) => Hit::TopLeft,
        (_, true, true, _) => Hit::TopRight,
        (true, _, _, true) => Hit::BottomLeft,
        (_, true, _, true) => Hit::BottomRight,
        (true, ..) => Hit::Left,
        (_, true, ..) => Hit::Right,
        (_, _, true, _) => Hit::Top,
        (.., true) => Hit::Bottom,
        _ if y < caption => Hit::Caption,
        _ => Hit::Client,
    }
}

/// A rectangle in screen pixels: left, top, right and bottom.
pub type ScreenRect = (i32, i32, i32, i32);

/// Where a window of `width` by `height` pixels asked for at `x`, `y` goes so the user can
/// reach it: where asked when enough of it is on a monitor's work area, else moved into the
/// first work area, as after a monitor was unplugged.
pub fn reachable_origin(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    areas: &[ScreenRect],
) -> (i32, i32) {
    const ENOUGH: i32 = 48;
    let on_screen = areas.iter().any(|&(left, top, right, bottom)| {
        let w = (x + width).min(right) - x.max(left);
        let h = (y + height).min(bottom) - y.max(top);
        w >= ENOUGH.min(width) && h >= ENOUGH.min(height) && y >= top - ENOUGH / 2
    });
    if on_screen || areas.is_empty() {
        return (x, y);
    }
    let (left, top, right, bottom) = areas[0];
    (
        x.clamp(left, (right - width).max(left)),
        y.clamp(top, (bottom - height).max(top)),
    )
}

/// A point in a window of `window` pixels, in the pixels of a frame of `frame` pixels, which
/// differ while a resize has not reached Android yet.
pub fn to_frame(x: i32, y: i32, window: (i32, i32), frame: (u32, u32)) -> (f32, f32) {
    let scale = |v: i32, from: i32, to: u32| {
        if from <= 0 {
            v as f32
        } else {
            v as f32 * to as f32 / from as f32
        }
    };
    (
        scale(x, window.0, frame.0).clamp(0.0, frame.0 as f32),
        scale(y, window.1, frame.1).clamp(0.0, frame.1 as f32),
    )
}

/// What the window manager asks of the widget windows.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowCommand {
    /// Open the widget's window, or move it to its placement.
    Place(Widget),
    /// Show what the widget shows now.
    Frame(WidgetFrame),
    /// Close the widget's window.
    Remove(i32),
}

/// What the widget windows report to the window manager.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowReport {
    /// The window is at `placement`, `width` by `height` pixels on a monitor of
    /// `density_dpi` (Android's density: 160 at a scale of 100%).
    Placed {
        widget_id: i32,
        placement: Placement,
        width: u32,
        height: u32,
        density_dpi: u32,
    },
    /// Pointer input in the widget, in its frame's pixels.
    Pointer {
        widget_id: i32,
        action: PointerAction,
        x: f32,
        y: f32,
    },
    /// The user chose Remove in the window's menu.
    Remove { widget_id: i32 },
}

/// The widget windows, as the watch drives them: the window manager's are Win32 windows on
/// a thread of their own ([`WindowsWidgetWindows`]); a test's records what it is asked.
pub trait WidgetWindows {
    fn command(&mut self, command: WindowCommand);
    /// What the windows reported since the last call.
    fn reports(&mut self) -> Vec<WindowReport>;
}

/// The command a change to the widgets is for the windows.
pub fn command_of(event: WidgetEvent) -> Option<WindowCommand> {
    match event {
        WidgetEvent::Placed(widget) => Some(WindowCommand::Place(widget)),
        WidgetEvent::Frame(frame) => Some(WindowCommand::Frame(frame)),
        WidgetEvent::Removed { widget_id } => Some(WindowCommand::Remove(widget_id)),
        WidgetEvent::Listed(_) => None,
    }
}

/// Moves still to send are dropped when a newer one of the same widget follows, so a slow
/// service never makes a drag lag behind.
pub fn coalesce(reports: Vec<WindowReport>) -> Vec<WindowReport> {
    let mut out: Vec<WindowReport> = Vec::with_capacity(reports.len());
    for report in reports {
        if let WindowReport::Pointer {
            widget_id,
            action: PointerAction::Move,
            ..
        } = &report
        {
            if let Some(WindowReport::Pointer {
                widget_id: last,
                action: PointerAction::Move,
                ..
            }) = out.last()
            {
                if last == widget_id {
                    out.pop();
                }
            }
        }
        out.push(report);
    }
    out
}

/// Passes what the windows reported to the service through `client`. A report the service
/// refuses is logged; the rest go on.
fn send_reports(
    client: &mut HostApiClient,
    reports: Vec<WindowReport>,
    logger: Option<&CompositorLogger>,
) -> Result<(), String> {
    for report in coalesce(reports) {
        let result = match report {
            WindowReport::Placed {
                widget_id,
                placement,
                width,
                height,
                density_dpi,
            } => client.place_widget(widget_id, placement, width, height, density_dpi),
            WindowReport::Pointer {
                widget_id,
                action,
                x,
                y,
            } => client.widget_pointer(widget_id, action, 0, x, y),
            WindowReport::Remove { widget_id } => {
                note(
                    logger,
                    &format!("removing widget {widget_id} from its menu"),
                );
                client.remove_widget(widget_id)
            }
        };
        match result {
            Ok(()) => {}
            Err(dsa_host_core::host_api::ClientError::Service { detail, .. }) => {
                note(logger, &format!("a widget's window: {detail}"));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

/// Shows the widgets of the user's Android until `stop`, reconnecting to the service
/// whenever the connection ends.
pub fn sync(
    mut connector: Box<dyn Connector>,
    windows: &mut dyn WidgetWindows,
    stop: Arc<AtomicBool>,
    logger: Option<Arc<CompositorLogger>>,
) {
    let mut last_error = String::new();
    let mut started_android = false;
    let mut shown: HashSet<i32> = HashSet::new();
    while !stop.load(Ordering::Relaxed) {
        let stream = HostApiClient::connect(&mut *connector, BUILD, ANSWER)
            .and_then(|client| client.watch_widgets());
        let requests = HostApiClient::connect(&mut *connector, BUILD, ANSWER);
        let (mut stream, mut requests) = match (stream, requests) {
            (Ok(stream), Ok(requests)) => (stream, requests),
            (Err(error), _) | (_, Err(error)) => {
                let error = error.to_string();
                if error != last_error {
                    note(
                        logger.as_deref(),
                        &format!("widgets cannot reach the service: {error}"),
                    );
                    last_error = error;
                }
                thread::sleep(RETRY);
                continue;
            }
        };
        last_error.clear();
        // Android starts with the user's sign-in, so no app launch waits for it to boot, and
        // the widgets the service remembers come back once it runs.
        if !started_android {
            started_android = true;
            match requests.status() {
                Ok(status) if starts_android_at_sign_in(status.state) => {
                    note(logger.as_deref(), "starting Android at sign-in");
                    if let Err(why) = requests.start() {
                        note(
                            logger.as_deref(),
                            &format!("Android did not start at sign-in: {why}"),
                        );
                    }
                }
                Ok(_) => {}
                Err(why) => note(logger.as_deref(), &format!("widgets: {why}")),
            }
        }
        // The windows of widgets that went while the connection was down close.
        let current: Vec<i32> = stream.current().iter().map(|w| w.widget_id).collect();
        for gone in shown
            .iter()
            .copied()
            .filter(|id| !current.contains(id))
            .collect::<Vec<_>>()
        {
            shown.remove(&gone);
            windows.command(WindowCommand::Remove(gone));
        }
        for widget in stream.current().to_vec() {
            shown.insert(widget.widget_id);
            windows.command(WindowCommand::Place(widget));
        }
        while !stop.load(Ordering::Relaxed) {
            if let Err(error) = send_reports(&mut requests, windows.reports(), logger.as_deref()) {
                note(
                    logger.as_deref(),
                    &format!("a widget's window did not reach the service: {error}"),
                );
                break;
            }
            match stream.next(POLL) {
                Ok(Some(event)) => {
                    match &event {
                        WidgetEvent::Placed(widget) => {
                            shown.insert(widget.widget_id);
                        }
                        WidgetEvent::Removed { widget_id } => {
                            shown.remove(widget_id);
                        }
                        _ => {}
                    }
                    if let Some(command) = command_of(event) {
                        windows.command(command);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    note(
                        logger.as_deref(),
                        &format!("the widgets' connection to the service ended: {error}"),
                    );
                    break;
                }
            }
        }
    }
    for id in shown {
        windows.command(WindowCommand::Remove(id));
    }
}

/// The widget windows: Win32 layered windows on a thread of their own, which the watch
/// drives through a channel.
#[cfg(windows)]
pub struct WindowsWidgetWindows {
    commands: std::sync::mpsc::Sender<WindowCommand>,
    reports: std::sync::mpsc::Receiver<WindowReport>,
    controller: Arc<std::sync::atomic::AtomicIsize>,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl WindowsWidgetWindows {
    /// Starts the windows' thread.
    pub fn open(logger: Option<Arc<CompositorLogger>>) -> Result<WindowsWidgetWindows, String> {
        let (commands, command_rx) = std::sync::mpsc::channel();
        let (report_tx, reports) = std::sync::mpsc::channel();
        let (ready_tx, ready) = std::sync::mpsc::channel();
        let controller = Arc::new(std::sync::atomic::AtomicIsize::new(0));
        let slot = Arc::clone(&controller);
        let thread = thread::Builder::new()
            .name("dsa-wm-widget-windows".into())
            .spawn(move || win::run(slot, command_rx, report_tx, ready_tx, logger))
            .map_err(|e| format!("could not start the widgets' thread: {e}"))?;
        match ready.recv() {
            Ok(Ok(())) => Ok(WindowsWidgetWindows {
                commands,
                reports,
                controller,
                thread: Some(thread),
            }),
            Ok(Err(why)) => {
                let _ = thread.join();
                Err(why)
            }
            Err(_) => {
                let _ = thread.join();
                Err("the widgets' thread ended before it was ready".into())
            }
        }
    }
}

#[cfg(windows)]
impl WidgetWindows for WindowsWidgetWindows {
    fn command(&mut self, command: WindowCommand) {
        if self.commands.send(command).is_ok() {
            win::wake(self.controller.load(Ordering::SeqCst));
        }
    }

    fn reports(&mut self) -> Vec<WindowReport> {
        self.reports.try_iter().collect()
    }
}

#[cfg(windows)]
impl Drop for WindowsWidgetWindows {
    fn drop(&mut self) {
        win::close(self.controller.load(Ordering::SeqCst));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::mpsc::{Receiver, Sender};
    use std::sync::Arc;

    use dsa_host_core::widgets::{Placement, Widget, WidgetFrame};
    use dsa_protocol::host_api::types::PointerAction;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
        MonitorFromWindow, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, MONITORINFO,
        MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{
        GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW,
        FindWindowW, GetMessageW, GetWindow, GetWindowRect, KillTimer, LoadCursorW, PostMessageW,
        PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SetTimer, SetWindowLongPtrW,
        SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow, GWLP_HWNDPARENT,
        GW_HWNDPREV, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT,
        HTTOP, HTTOPLEFT, HTTOPRIGHT, HWND_BOTTOM, HWND_MESSAGE, IDC_ARROW, MA_NOACTIVATE,
        MINMAXINFO, MSG, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWNOACTIVATE,
        ULW_ALPHA, WINDOWPOS, WM_APP, WM_CAPTURECHANGED, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
        WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCHITTEST, WM_SIZE, WM_TIMER,
        WM_WINDOWPOSCHANGING, WNDCLASSEXW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible, GW_HWNDNEXT};

    use std::time::Instant;

    use super::{
        android_density, frame_overdue, hit_test, min_window_size, placement_of, reachable_origin,
        right_above_desktop, to_frame, window_size, window_styles, z_place, Below, Hit,
        WindowCommand, WindowReport, ZPlace, NO_FRAME, NO_FRAME_CHECK, RESTACK_CHECK,
    };
    use crate::compositor::log::{note, CompositorLogger};
    use crate::compositor::widget_menu::{self, MenuChoice};

    /// The message that tells the controller window commands are waiting.
    const WM_COMMANDS: u32 = WM_APP + 1;

    /// The pointer left the window (`WM_MOUSELEAVE`, which windows-sys keeps with the common
    /// controls).
    const WM_MOUSELEAVE: u32 = 0x02A3;

    struct Window {
        widget: Widget,
        frame: Option<WidgetFrame>,
        hover: bool,
        pressed: bool,
        /// Since when the window waits for its first image; None once one came.
        waiting_since: Option<Instant>,
        /// Whether the wait was logged already.
        noted: bool,
    }

    /// The controller's timer that checks for windows still without an image.
    const NO_FRAME_TIMER: usize = 1;

    /// The controller's timer that keeps the windows right above the desktop.
    const RESTACK_TIMER: usize = 2;

    /// What the last drawing of a window handed the system, so a test can tell a window that
    /// shows something from one that shows nothing.
    #[cfg(test)]
    #[derive(Debug, Clone, Copy)]
    pub(super) struct Presented {
        pub hwnd: isize,
        /// Whether UpdateLayeredWindow took the picture.
        pub committed: bool,
        pub width: i32,
        pub height: i32,
        /// How many of its pixels are at least half opaque.
        pub opaque: usize,
        /// Whether the window was shown already when it got the picture.
        pub visible: bool,
    }

    #[cfg(test)]
    pub(super) static PRESENTED: std::sync::Mutex<Vec<Presented>> =
        std::sync::Mutex::new(Vec::new());

    struct State {
        commands: Receiver<WindowCommand>,
        reports: Sender<WindowReport>,
        windows: HashMap<isize, Window>,
        by_id: HashMap<i32, isize>,
        class: Vec<u16>,
        taskbar_created: u32,
        logger: Option<Arc<CompositorLogger>>,
        decoder: Option<super::png::Decoder>,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
        STATE.with(|state| match state.try_borrow_mut() {
            Ok(mut state) => state.as_mut().map(f),
            Err(_) => None,
        })
    }

    pub(super) fn wake(controller: isize) {
        if controller != 0 {
            // SAFETY: posts to the controller window, which may have gone; then it fails.
            unsafe { PostMessageW(controller as HWND, WM_COMMANDS, 0, 0) };
        }
    }

    pub(super) fn close(controller: isize) {
        if controller != 0 {
            // SAFETY: as above.
            unsafe { PostMessageW(controller as HWND, WM_CLOSE, 0, 0) };
        }
    }

    /// The shell's desktop window, which widget windows belong to so they stay on the
    /// desktop when the user shows it.
    fn desktop_owner() -> HWND {
        let class = wide("Progman");
        // SAFETY: a NUL-terminated class name.
        unsafe { FindWindowW(class.as_ptr(), null()) }
    }

    fn lo(l: LPARAM) -> i32 {
        (l & 0xffff) as u16 as i16 as i32
    }

    fn hi(l: LPARAM) -> i32 {
        ((l >> 16) & 0xffff) as u16 as i16 as i32
    }

    fn rect_of(hwnd: HWND) -> RECT {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: a window of this thread and a rectangle to fill.
        unsafe { GetWindowRect(hwnd, &mut rect) };
        rect
    }

    fn dpi_of(hwnd: HWND) -> u32 {
        // SAFETY: a window of this thread.
        match unsafe { GetDpiForWindow(hwnd) } {
            0 => 96,
            dpi => dpi,
        }
    }

    fn work_area(hwnd: HWND) -> Option<super::ScreenRect> {
        // SAFETY: a window of this thread, and a structure of the right size to fill.
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return None;
            }
            let w = info.rcWork;
            Some((w.left, w.top, w.right, w.bottom))
        }
    }

    /// Opens the controller window, says whether it did, and runs the windows' messages
    /// until it is closed.
    pub(super) fn run(
        slot: Arc<AtomicIsize>,
        commands: Receiver<WindowCommand>,
        reports: Sender<WindowReport>,
        ready: Sender<Result<(), String>>,
        logger: Option<Arc<CompositorLogger>>,
    ) {
        // Every window of this thread follows its monitor's scale.
        // SAFETY: changes this thread's DPI awareness only.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let _com = crate::start_menu::ComApartment::enter();
        let decoder = match super::png::Decoder::new() {
            Ok(decoder) => Some(decoder),
            Err(why) => {
                note(logger.as_deref(), &format!("widgets show no image: {why}"));
                None
            }
        };
        let controller_class = wide(&format!("DsaWmWidgets_{}", std::process::id()));
        let class = wide(&format!("DsaWmWidget_{}", std::process::id()));
        // SAFETY: the module handle of this process.
        let instance = unsafe { GetModuleHandleW(null()) };
        for (name, proc_) in [
            (&controller_class, controller_proc as WndProc),
            (&class, widget_proc as WndProc),
        ] {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(proc_),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: null_mut(),
                // SAFETY: a system cursor.
                hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: name.as_ptr(),
                hIconSm: null_mut(),
            };
            // SAFETY: the names outlive the classes, which live as long as the process.
            unsafe { RegisterClassExW(&wc) };
        }
        let taskbar = wide("TaskbarCreated");
        // SAFETY: a NUL-terminated message name.
        let taskbar_created = unsafe { RegisterWindowMessageW(taskbar.as_ptr()) };
        STATE.with(|state| {
            *state.borrow_mut() = Some(State {
                commands,
                reports,
                windows: HashMap::new(),
                by_id: HashMap::new(),
                class,
                taskbar_created,
                logger,
                decoder,
            })
        });
        // SAFETY: a message-only window, never shown.
        let controller = unsafe {
            CreateWindowExW(
                0,
                controller_class.as_ptr(),
                controller_class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                instance,
                null(),
            )
        };
        if controller.is_null() {
            let _ = ready.send(Err(format!(
                "could not open the widgets' window: {}",
                std::io::Error::last_os_error()
            )));
            return;
        }
        // SAFETY: a timer of the controller window, killed when it closes.
        unsafe {
            SetTimer(
                controller,
                NO_FRAME_TIMER,
                NO_FRAME_CHECK.as_millis() as u32,
                None,
            );
            SetTimer(
                controller,
                RESTACK_TIMER,
                RESTACK_CHECK.as_millis() as u32,
                None,
            );
        }
        slot.store(controller as isize, Ordering::SeqCst);
        let _ = ready.send(Ok(()));
        // SAFETY: the usual message loop of this thread's windows.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        slot.store(0, Ordering::SeqCst);
        STATE.with(|state| *state.borrow_mut() = None);
    }

    type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

    unsafe extern "system" fn controller_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_COMMANDS => {
                apply_commands();
                0
            }
            WM_TIMER if wparam == NO_FRAME_TIMER => {
                note_missing_frames();
                0
            }
            WM_TIMER if wparam == RESTACK_TIMER => {
                keep_above_desktop();
                0
            }
            WM_CLOSE => {
                let windows: Vec<isize> =
                    with_state(|s| s.windows.keys().copied().collect()).unwrap_or_default();
                for window in windows {
                    // SAFETY: windows of this thread.
                    unsafe { DestroyWindow(window as HWND) };
                }
                // SAFETY: the controller is this thread's, and so are its timers.
                unsafe {
                    KillTimer(hwnd, NO_FRAME_TIMER);
                    KillTimer(hwnd, RESTACK_TIMER);
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_DESTROY => {
                // SAFETY: ends this thread's message loop.
                unsafe { PostQuitMessage(0) };
                0
            }
            // SAFETY: the default handling of every other message.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn apply_commands() {
        loop {
            let Some(Some(command)) = with_state(|s| s.commands.try_recv().ok()) else {
                return;
            };
            match command {
                WindowCommand::Place(widget) => place(widget),
                WindowCommand::Frame(frame) => {
                    let hwnd = with_state(|s| {
                        let hwnd = *s.by_id.get(&frame.widget_id)?;
                        let window = s.windows.get_mut(&hwnd)?;
                        if window.noted {
                            note(
                                s.logger.as_deref(),
                                &format!(
                                    "widget {}: its first image came, {}x{} pixels",
                                    frame.widget_id, frame.width, frame.height
                                ),
                            );
                        }
                        window.waiting_since = None;
                        window.noted = false;
                        window.frame = Some(frame);
                        Some(hwnd)
                    })
                    .flatten();
                    if let Some(hwnd) = hwnd {
                        present(hwnd as HWND);
                    }
                }
                WindowCommand::Remove(widget_id) => {
                    let hwnd = with_state(|s| {
                        let hwnd = s.by_id.remove(&widget_id)?;
                        s.windows.remove(&hwnd);
                        Some(hwnd)
                    })
                    .flatten();
                    if let Some(hwnd) = hwnd {
                        // SAFETY: a window of this thread.
                        unsafe { DestroyWindow(hwnd as HWND) };
                    }
                }
            }
        }
    }

    /// Logs each widget window that has waited [`NO_FRAME`] for its first image from Android,
    /// once, so a widget that shows nothing is visible in the log.
    fn note_missing_frames() {
        let now = Instant::now();
        with_state(|s| {
            for (hwnd, window) in s.windows.iter_mut() {
                if !frame_overdue(window.waiting_since, window.noted, now) {
                    continue;
                }
                window.noted = true;
                let waited = window
                    .waiting_since
                    .map(|since| now.saturating_duration_since(since).as_secs())
                    .unwrap_or(NO_FRAME.as_secs());
                let rect = rect_of(*hwnd as HWND);
                note(
                    s.logger.as_deref(),
                    &format!(
                        "widget {} ({}) has had no image from Android for {waited} s; its window of {}x{} pixels shows only its placeholder. The service's and the agent's logs say why",
                        window.widget.widget_id,
                        window.widget.provider.provider,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                    ),
                );
            }
        });
    }

    /// The desktop's own window: the shell's window that holds the desktop icons, which a
    /// wallpaper slideshow or "show desktop" moves out of `Progman` into a `WorkerW`.
    pub(super) fn desktop_window() -> Option<HWND> {
        let progman = desktop_owner();
        let defview = wide("SHELLDLL_DefView");
        let worker = wide("WorkerW");
        // SAFETY: walks the top-level windows by class; the names outlive the calls.
        unsafe {
            if !progman.is_null()
                && !FindWindowExW(progman, null_mut(), defview.as_ptr(), null()).is_null()
            {
                return Some(progman);
            }
            let mut after: HWND = null_mut();
            loop {
                after = FindWindowExW(null_mut(), after, worker.as_ptr(), null());
                if after.is_null() {
                    break;
                }
                if !FindWindowExW(after, null_mut(), defview.as_ptr(), null()).is_null() {
                    return Some(after);
                }
            }
        }
        (!progman.is_null()).then_some(progman)
    }

    /// Puts every widget window that the shell left under the desktop, or above an app's
    /// window, right above the desktop again ([`right_above_desktop`]). The shell restacks
    /// its desktop window without moving the widget windows, so they hear nothing of it.
    fn keep_above_desktop() {
        let Some(desktop) = desktop_window() else {
            return;
        };
        let ours: Vec<isize> =
            with_state(|s| s.windows.keys().copied().collect()).unwrap_or_default();
        for &own in &ours {
            let rect = rect_of(own as HWND);
            let mut below = Vec::new();
            // SAFETY: walks the z-order down from a window of this thread; a window that goes
            // meanwhile ends the walk.
            unsafe {
                let mut next = GetWindow(own as HWND, GW_HWNDNEXT);
                while !next.is_null() && below.len() < 4096 {
                    let other = rect_of(next);
                    let overlaps = other.left < rect.right
                        && rect.left < other.right
                        && other.top < rect.bottom
                        && rect.top < other.bottom;
                    below.push(Below {
                        hwnd: next as isize,
                        covers: overlaps && IsWindowVisible(next) != 0 && IsIconic(next) == 0,
                    });
                    next = GetWindow(next, GW_HWNDNEXT);
                }
            }
            if !right_above_desktop(&below, desktop as isize, &ours) {
                // SAFETY: a window of this thread; WM_WINDOWPOSCHANGING puts it right above
                // the desktop.
                unsafe {
                    SetWindowPos(
                        own as HWND,
                        HWND_BOTTOM,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    );
                }
            }
        }
    }

    /// Where `hwnd` goes in the z-order: right above the desktop ([`z_place`]).
    fn z_place_of(hwnd: HWND) -> ZPlace {
        let desktop = desktop_window();
        // SAFETY: reads the window right above the desktop's.
        let above = desktop.map(|d| unsafe { GetWindow(d, GW_HWNDPREV) });
        z_place(
            desktop.map(|d| d as isize),
            above.filter(|a| !a.is_null()).map(|a| a as isize),
            hwnd as isize,
        )
    }

    /// Opens the widget's window at its placement, or moves the open one there, and tells
    /// the service the window's size, which is how Android learns what size to draw at.
    fn place(widget: Widget) {
        let existing = with_state(|s| s.by_id.get(&widget.widget_id).copied()).flatten();
        let hwnd = match existing {
            Some(hwnd) => {
                with_state(|s| {
                    if let Some(window) = s.windows.get_mut(&hwnd) {
                        window.widget = widget.clone();
                    }
                });
                hwnd as HWND
            }
            None => {
                let Some(class) = with_state(|s| s.class.clone()) else {
                    return;
                };
                let (style, ex_style) = window_styles();
                let (width, height) = window_size(&widget.placement, 96);
                // SAFETY: a popup window owned by the desktop; the names outlive the call.
                let hwnd = unsafe {
                    CreateWindowExW(
                        ex_style,
                        class.as_ptr(),
                        class.as_ptr(),
                        style,
                        widget.placement.x,
                        widget.placement.y,
                        width,
                        height,
                        desktop_owner(),
                        null_mut(),
                        GetModuleHandleW(null()),
                        null(),
                    )
                };
                if hwnd.is_null() {
                    with_state(|s| {
                        note(
                            s.logger.as_deref(),
                            &format!(
                                "the window of widget {} did not open: {}",
                                widget.widget_id,
                                std::io::Error::last_os_error()
                            ),
                        )
                    });
                    return;
                }
                with_state(|s| {
                    s.by_id.insert(widget.widget_id, hwnd as isize);
                    s.windows.insert(
                        hwnd as isize,
                        Window {
                            widget: widget.clone(),
                            frame: None,
                            hover: false,
                            pressed: false,
                            waiting_since: Some(Instant::now()),
                            noted: false,
                        },
                    );
                });
                hwnd
            }
        };
        let dpi = dpi_of(hwnd);
        let (width, height) = window_size(&widget.placement, dpi);
        let (x, y) = match work_area(hwnd) {
            Some(area) => reachable_origin(
                widget.placement.x,
                widget.placement.y,
                width,
                height,
                &[area],
            ),
            None => (widget.placement.x, widget.placement.y),
        };
        // SAFETY: a window of this thread; it goes right above the desktop, where
        // WM_WINDOWPOSCHANGING puts it.
        unsafe { SetWindowPos(hwnd, HWND_BOTTOM, x, y, width, height, SWP_NOACTIVATE) };
        // A layered window shows nothing at all until it is given a picture, so it gets its
        // placeholder before it is shown, never after.
        present(hwnd);
        // SAFETY: as above.
        unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        keep_above_desktop();
        report_placement(hwnd);
    }

    fn report_placement(hwnd: HWND) {
        let rect = rect_of(hwnd);
        let dpi = dpi_of(hwnd);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        with_state(|s| {
            let Some(window) = s.windows.get(&(hwnd as isize)) else {
                return;
            };
            let placement = placement_of(rect.left, rect.top, width, height, dpi);
            let _ = s.reports.send(WindowReport::Placed {
                widget_id: window.widget.widget_id,
                placement,
                width: width.max(1) as u32,
                height: height.max(1) as u32,
                density_dpi: android_density(dpi),
            });
        });
    }

    /// Draws the widget's latest frame, scaled to its window, with the move band while the
    /// pointer is over it. Pixels the widget leaves clear stay just visible enough to be
    /// clicked, so the whole window can be grabbed.
    fn present(hwnd: HWND) {
        let rect = rect_of(hwnd);
        let (width, height) = (
            (rect.right - rect.left).max(1),
            (rect.bottom - rect.top).max(1),
        );
        let dpi = dpi_of(hwnd);
        let pixels = with_state(|s| {
            let window = s.windows.get(&(hwnd as isize))?;
            let decoded = match (&window.frame, &s.decoder) {
                (Some(frame), Some(decoder)) => decoder
                    .decode(&frame.png, width as u32, height as u32)
                    .map_err(|why| note(s.logger.as_deref(), &format!("a widget's image: {why}")))
                    .ok(),
                _ => None,
            };
            let decoded = match decoded {
                Some(pixels) if super::all_clear(&pixels) => {
                    note(
                        s.logger.as_deref(),
                        &format!(
                            "widget {}: Android sent an image with nothing in it; showing the placeholder",
                            window.widget.widget_id
                        ),
                    );
                    None
                }
                other => other,
            };
            let mut pixels = decoded
                .unwrap_or_else(|| super::placeholder(width as usize, height as usize, dpi));
            finish(
                &mut pixels,
                width as usize,
                height as usize,
                dpi,
                window.hover,
            );
            Some(pixels)
        })
        .flatten();
        let Some(pixels) = pixels else {
            return;
        };
        let mut committed = false;
        let mut failure = None;
        // SAFETY: a DIB section of the window's size, filled from `pixels`, selected into a
        // memory DC for UpdateLayeredWindow and freed after.
        unsafe {
            let screen = GetDC(null_mut());
            let memory = CreateCompatibleDC(screen);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let mut bits: *mut core::ffi::c_void = null_mut();
            let bitmap = CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if !bitmap.is_null() && !bits.is_null() {
                std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
                let old = SelectObject(memory, bitmap);
                let size = SIZE {
                    cx: width,
                    cy: height,
                };
                let source = POINT { x: 0, y: 0 };
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                committed = UpdateLayeredWindow(
                    hwnd,
                    screen,
                    null(),
                    &size,
                    memory,
                    &source,
                    0,
                    &blend,
                    ULW_ALPHA,
                ) != 0;
                if !committed {
                    failure = Some(std::io::Error::last_os_error());
                }
                SelectObject(memory, old);
                DeleteObject(bitmap);
            } else {
                failure = Some(std::io::Error::last_os_error());
            }
            DeleteDC(memory);
            ReleaseDC(null_mut(), screen);
        }
        if let Some(why) = failure {
            // Without a picture a layered window is not drawn at all, so this is logged.
            with_state(|s| {
                let id = s.windows.get(&(hwnd as isize)).map(|w| w.widget.widget_id);
                note(
                    s.logger.as_deref(),
                    &format!(
                        "widget {}: its window of {width}x{height} pixels could not be drawn: {why}",
                        id.unwrap_or_default()
                    ),
                );
            });
        }
        record(hwnd, committed, width, height, &pixels);
    }

    #[cfg(test)]
    fn record(hwnd: HWND, committed: bool, width: i32, height: i32, pixels: &[u8]) {
        if let Ok(mut presented) = PRESENTED.lock() {
            presented.push(Presented {
                hwnd: hwnd as isize,
                committed,
                width,
                height,
                opaque: pixels.chunks_exact(4).filter(|p| p[3] >= 0x80).count(),
                // SAFETY: a window of this thread.
                visible: unsafe { IsWindowVisible(hwnd) } != 0,
            });
        }
    }

    #[cfg(not(test))]
    fn record(_: HWND, _: bool, _: i32, _: i32, _: &[u8]) {}

    /// Keeps every pixel clickable and draws the move band while the pointer is over the
    /// window.
    fn finish(pixels: &mut [u8], width: usize, height: usize, dpi: u32, hover: bool) {
        for pixel in pixels.chunks_exact_mut(4) {
            if pixel[3] == 0 {
                pixel[3] = 1;
            }
        }
        if hover {
            let band =
                (super::px_of_dip(super::CAPTION_DIP as u32, dpi).max(1) as usize).min(height);
            for pixel in pixels[..band * width * 4].chunks_exact_mut(4) {
                // Over the image, a dark band at half: premultiplied blend.
                for channel in &mut pixel[..3] {
                    *channel /= 2;
                }
                pixel[3] = pixel[3] / 2 + 0x60;
            }
        }
    }

    unsafe extern "system" fn widget_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let key = hwnd as isize;
        match msg {
            WM_NCHITTEST => {
                let rect = rect_of(hwnd);
                let (x, y) = (lo(lparam) - rect.left, hi(lparam) - rect.top);
                let resize_mode =
                    with_state(|s| s.windows.get(&key).map(|w| w.widget.provider.resize_mode))
                        .flatten()
                        .unwrap_or(0);
                let hit = hit_test(
                    x,
                    y,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    dpi_of(hwnd),
                    resize_mode,
                );
                (match hit {
                    Hit::Client => HTCLIENT,
                    Hit::Caption => HTCAPTION,
                    Hit::Left => HTLEFT,
                    Hit::Right => HTRIGHT,
                    Hit::Top => HTTOP,
                    Hit::Bottom => HTBOTTOM,
                    Hit::TopLeft => HTTOPLEFT,
                    Hit::TopRight => HTTOPRIGHT,
                    Hit::BottomLeft => HTBOTTOMLEFT,
                    Hit::BottomRight => HTBOTTOMRIGHT,
                }) as LRESULT
            }
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_WINDOWPOSCHANGING => {
                // Kept right above the desktop, below every other window, whatever raises it.
                let pos = lparam as *mut WINDOWPOS;
                // SAFETY: the system passes a WINDOWPOS it reads back.
                unsafe {
                    if !pos.is_null() && (*pos).flags & SWP_NOZORDER == 0 {
                        match z_place_of(hwnd) {
                            ZPlace::After(above) => (*pos).hwndInsertAfter = above as HWND,
                            ZPlace::Keep => (*pos).flags |= SWP_NOZORDER,
                            ZPlace::Bottom => (*pos).hwndInsertAfter = HWND_BOTTOM,
                        }
                    }
                }
                0
            }
            WM_GETMINMAXINFO => {
                let min = with_state(|s| {
                    s.windows
                        .get(&key)
                        .map(|w| min_window_size(&w.widget.provider, dpi_of(hwnd)))
                })
                .flatten();
                if let Some((width, height)) = min {
                    let info = lparam as *mut MINMAXINFO;
                    // SAFETY: the system passes a MINMAXINFO it reads back.
                    unsafe {
                        if !info.is_null() {
                            (*info).ptMinTrackSize = POINT {
                                x: width,
                                y: height,
                            };
                        }
                    }
                }
                0
            }
            WM_SIZE => {
                present(hwnd);
                0
            }
            WM_ENTERSIZEMOVE => 0,
            WM_EXITSIZEMOVE => {
                report_placement(hwnd);
                0
            }
            WM_DPICHANGED => {
                let suggested = lparam as *const RECT;
                // SAFETY: the system passes the rectangle it suggests for the new scale.
                unsafe {
                    if !suggested.is_null() {
                        let r = *suggested;
                        SetWindowPos(
                            hwnd,
                            null_mut(),
                            r.left,
                            r.top,
                            r.right - r.left,
                            r.bottom - r.top,
                            SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                    }
                }
                report_placement(hwnd);
                0
            }
            WM_MOUSEMOVE => {
                let first = with_state(|s| {
                    s.windows
                        .get_mut(&key)
                        .map(|w| !std::mem::replace(&mut w.hover, true))
                })
                .flatten()
                .unwrap_or(false);
                if first {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: asks for WM_MOUSELEAVE for this window.
                    unsafe { TrackMouseEvent(&mut track) };
                    present(hwnd);
                }
                pointer(hwnd, PointerAction::Move, lparam);
                0
            }
            WM_MOUSELEAVE => {
                with_state(|s| {
                    if let Some(w) = s.windows.get_mut(&key) {
                        w.hover = false;
                    }
                });
                present(hwnd);
                0
            }
            WM_LBUTTONDOWN => {
                // SAFETY: the window takes the mouse until the button is released.
                unsafe { SetCapture(hwnd) };
                with_state(|s| {
                    if let Some(w) = s.windows.get_mut(&key) {
                        w.pressed = true;
                    }
                });
                pointer(hwnd, PointerAction::Down, lparam);
                0
            }
            WM_LBUTTONUP => {
                pointer(hwnd, PointerAction::Up, lparam);
                with_state(|s| {
                    if let Some(w) = s.windows.get_mut(&key) {
                        w.pressed = false;
                    }
                });
                // SAFETY: gives the mouse back.
                unsafe { ReleaseCapture() };
                0
            }
            WM_CAPTURECHANGED => {
                let was_pressed = with_state(|s| {
                    s.windows
                        .get_mut(&key)
                        .map(|w| std::mem::replace(&mut w.pressed, false))
                })
                .flatten()
                .unwrap_or(false);
                if was_pressed {
                    pointer(hwnd, PointerAction::Cancel, 0);
                }
                0
            }
            WM_CONTEXTMENU => {
                context_menu(hwnd, lparam);
                0
            }
            WM_CLOSE => 0,
            m if m != 0 && with_state(|s| s.taskbar_created).unwrap_or(0) == m => {
                // The shell restarted: the window belongs to its new desktop.
                // SAFETY: a window of this thread.
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, desktop_owner() as isize);
                    SetWindowPos(
                        hwnd,
                        HWND_BOTTOM,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    );
                }
                0
            }
            // SAFETY: the default handling of every other message.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    /// Shows the window's menu (Resize, Remove) where the user right-clicked, or at the
    /// window's corner for the menu key, and does what the user chose.
    fn context_menu(hwnd: HWND, lparam: LPARAM) {
        let key = hwnd as isize;
        let Some((provider, placement, widget_id)) = with_state(|s| {
            s.windows.get(&key).map(|w| {
                (
                    w.widget.provider.clone(),
                    w.widget.placement,
                    w.widget.widget_id,
                )
            })
        })
        .flatten() else {
            return;
        };
        let rect = rect_of(hwnd);
        let (x, y) = if lparam as u32 == u32::MAX {
            (rect.left, rect.top)
        } else {
            (lo(lparam), hi(lparam))
        };
        let dpi = dpi_of(hwnd);
        // The size shown as current is the window's own, which a drag may have changed.
        let shown = placement_of(
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            dpi,
        );
        let placement = Placement {
            width_dp: shown.width_dp,
            height_dp: shown.height_dp,
            ..placement
        };
        match widget_menu::show(hwnd as isize, x, y, &provider, &placement) {
            Some(MenuChoice::Resize {
                width_dp,
                height_dp,
            }) => {
                let (width, height) = window_size(
                    &Placement {
                        width_dp,
                        height_dp,
                        ..placement
                    },
                    dpi,
                );
                // SAFETY: a window of this thread; it keeps its place and its z-order.
                unsafe {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        0,
                        0,
                        width,
                        height,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
                    );
                }
                report_placement(hwnd);
            }
            Some(MenuChoice::Remove) => {
                with_state(|s| {
                    let _ = s.reports.send(WindowReport::Remove { widget_id });
                });
            }
            None => {}
        }
    }

    /// Reports pointer input inside the widget, in its frame's pixels. Moves count only while
    /// the button is down.
    fn pointer(hwnd: HWND, action: PointerAction, lparam: LPARAM) {
        let rect = rect_of(hwnd);
        let window = (rect.right - rect.left, rect.bottom - rect.top);
        let (x, y) = (lo(lparam), hi(lparam));
        with_state(|s| {
            let Some(w) = s.windows.get(&(hwnd as isize)) else {
                return;
            };
            if action == PointerAction::Move && !w.pressed {
                return;
            }
            let frame = w
                .frame
                .as_ref()
                .map(|f| (f.width, f.height))
                .unwrap_or((window.0.max(1) as u32, window.1.max(1) as u32));
            let (fx, fy) = to_frame(x, y, window, frame);
            let _ = s.reports.send(WindowReport::Pointer {
                widget_id: w.widget.widget_id,
                action,
                x: fx,
                y: fy,
            });
        });
    }
}

/// Decodes widget frames, and the icons of the apps that run in the background, with the
/// Windows Imaging Component.
#[cfg(windows)]
pub(crate) mod png {
    use windows::core::GUID;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, GUID_WICPixelFormat32bppPBGRA,
        IWICBitmapSource, IWICImagingFactory, IWICPalette, WICBitmapDitherTypeNone,
        WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

    pub(crate) struct Decoder {
        factory: IWICImagingFactory,
    }

    impl Decoder {
        pub(crate) fn new() -> Result<Decoder, String> {
            // SAFETY: COM is entered on this thread.
            let factory: IWICImagingFactory =
                unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
                    .map_err(|e| format!("the imaging component is not there: {e}"))?;
            Ok(Decoder { factory })
        }

        /// The PNG image `png`, scaled to `width` by `height`, as premultiplied BGRA rows.
        pub(crate) fn decode(
            &self,
            png: &[u8],
            width: u32,
            height: u32,
        ) -> Result<Vec<u8>, String> {
            self.decode_as(png, width, height, &GUID_WICPixelFormat32bppPBGRA)
        }

        /// The same with straight alpha, as an icon's colour bitmap takes it.
        pub(crate) fn decode_straight(
            &self,
            png: &[u8],
            width: u32,
            height: u32,
        ) -> Result<Vec<u8>, String> {
            self.decode_as(png, width, height, &GUID_WICPixelFormat32bppBGRA)
        }

        fn decode_as(
            &self,
            png: &[u8],
            width: u32,
            height: u32,
            format: &GUID,
        ) -> Result<Vec<u8>, String> {
            let fail = |what: &str, e: windows::core::Error| format!("{what}: {e}");
            // SAFETY: COM calls on interfaces this thread made, with buffers that outlive them.
            unsafe {
                let stream = self
                    .factory
                    .CreateStream()
                    .map_err(|e| fail("reading the image", e))?;
                stream
                    .InitializeFromMemory(png)
                    .map_err(|e| fail("reading the image", e))?;
                let decoder = self
                    .factory
                    .CreateDecoderFromStream(
                        &stream,
                        std::ptr::null(),
                        WICDecodeMetadataCacheOnDemand,
                    )
                    .map_err(|e| fail("the image is not a PNG", e))?;
                let frame = decoder
                    .GetFrame(0)
                    .map_err(|e| fail("the image has no frame", e))?;
                let converter = self
                    .factory
                    .CreateFormatConverter()
                    .map_err(|e| fail("converting the image", e))?;
                converter
                    .Initialize(
                        &frame,
                        format,
                        WICBitmapDitherTypeNone,
                        None::<&IWICPalette>,
                        0.0,
                        WICBitmapPaletteTypeCustom,
                    )
                    .map_err(|e| fail("converting the image", e))?;
                let scaler = self
                    .factory
                    .CreateBitmapScaler()
                    .map_err(|e| fail("scaling the image", e))?;
                scaler
                    .Initialize(&converter, width, height, WICBitmapInterpolationModeFant)
                    .map_err(|e| fail("scaling the image", e))?;
                let source: IWICBitmapSource = scaler.into();
                let stride = width * 4;
                let mut pixels = vec![0u8; (stride * height) as usize];
                source
                    .CopyPixels(std::ptr::null(), stride, &mut pixels)
                    .map_err(|e| fail("copying the image", e))?;
                Ok(pixels)
            }
        }
    }
}

/// Whether the window manager starts Android at sign-in, seeing it in `state`: when it is not
/// running. One that is starting, running or dozing is left as it is.
pub fn starts_android_at_sign_in(state: SessionState) -> bool {
    state == SessionState::Stopped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr10_the_window_manager_starts_a_stopped_android_at_sign_in() {
        assert!(starts_android_at_sign_in(SessionState::Stopped));
        for running in [
            SessionState::Starting,
            SessionState::Active,
            SessionState::Dozing,
            SessionState::Frozen,
        ] {
            assert!(!starts_android_at_sign_in(running), "{running:?}");
        }
    }
    use dsa_host_core::widgets::WidgetProvider;

    fn clock() -> WidgetProvider {
        WidgetProvider {
            provider: "com.example.clock/.ClockWidget".into(),
            package_name: "com.example.clock".into(),
            label: "Clock".into(),
            preview_png: Vec::new(),
            min_width_dp: 110,
            min_height_dp: 40,
            default_width_dp: 180,
            default_height_dp: 110,
            max_width_dp: 0,
            max_height_dp: 0,
            resize_mode: 3,
            needs_configuration: false,
            app_label: "Clock App".into(),
        }
    }

    #[test]
    fn fr12_a_widget_window_without_an_image_shows_a_visible_placeholder() {
        let (width, height) = (600usize, 340usize);
        let pixels = placeholder(width, height, 192);
        assert_eq!(pixels.len(), width * height * 4);
        assert!(!all_clear(&pixels));
        let alpha = |x: usize, y: usize| pixels[(y * width + x) * 4 + 3];
        // The middle and every edge's midpoint are mostly opaque, so the window is never a
        // see-through hole on the desktop.
        for (x, y) in [
            (width / 2, height / 2),
            (0, height / 2),
            (width - 1, height / 2),
            (width / 2, 0),
            (width / 2, height - 1),
        ] {
            assert!(alpha(x, y) >= 0xc0, "alpha at {x},{y} is {}", alpha(x, y));
        }
        // The corners are rounded off.
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(width - 1, height - 1), 0);
        // Premultiplied: no colour exceeds its alpha.
        assert!(pixels
            .chunks_exact(4)
            .all(|p| p[..3].iter().all(|&c| c <= p[3])));
        // A tiny window still gets one.
        assert!(!all_clear(&placeholder(3, 2, 96)));
    }

    #[test]
    fn fr12_an_image_with_nothing_in_it_is_told_from_one_with_something_drawn() {
        assert!(all_clear(&[0, 0, 0, 0, 9, 9, 9, 0]));
        assert!(!all_clear(&[0, 0, 0, 0, 0, 0, 0, 1]));
    }

    #[test]
    fn fr12_a_widget_windows_size_follows_its_monitors_scale() {
        let placement = Placement {
            x: 10,
            y: 20,
            width_dp: 180,
            height_dp: 110,
        };
        assert_eq!(window_size(&placement, 96), (180, 110));
        assert_eq!(window_size(&placement, 144), (270, 165));
        assert_eq!(placement_of(10, 20, 270, 165, 144), placement);
        assert_eq!(android_density(96), 160);
        assert_eq!(android_density(144), 240);
        assert_eq!(android_density(120), 200);
        assert_eq!(dip_of_px(361, 240), 144);
        assert_eq!(min_window_size(&clock(), 192), (220, 80));
    }

    #[test]
    fn fr12_a_widget_window_is_kept_off_the_taskbar_and_alt_tab_and_never_takes_the_focus() {
        let (style, ex) = window_styles();
        assert_eq!(style, 0x8000_0000, "a popup with no frame");
        assert_ne!(ex & 0x80, 0, "a tool window: not in the taskbar or Alt+Tab");
        assert_ne!(
            ex & 0x0008_0000,
            0,
            "layered: see-through where the widget is"
        );
        assert_ne!(ex & 0x0800_0000, 0, "never takes the focus");
        assert_eq!(ex & 0x0000_0008, 0, "never topmost");
    }

    #[test]
    fn fr12_a_widget_window_is_moved_by_its_top_band_and_resized_by_its_edges() {
        let (w, h) = (200, 120);
        assert_eq!(hit_test(100, 60, w, h, 96, 3), Hit::Client);
        assert_eq!(hit_test(100, 10, w, h, 96, 3), Hit::Caption);
        assert_eq!(hit_test(2, 60, w, h, 96, 3), Hit::Left);
        assert_eq!(hit_test(198, 60, w, h, 96, 3), Hit::Right);
        assert_eq!(hit_test(100, 118, w, h, 96, 3), Hit::Bottom);
        assert_eq!(hit_test(100, 2, w, h, 96, 3), Hit::Top);
        assert_eq!(hit_test(198, 118, w, h, 96, 3), Hit::BottomRight);
        assert_eq!(hit_test(1, 1, w, h, 96, 3), Hit::TopLeft);
        // Only the edges the widget may be resized along resize it.
        assert_eq!(hit_test(2, 60, w, h, 96, 2), Hit::Client);
        assert_eq!(hit_test(100, 118, w, h, 96, 1), Hit::Client);
        assert_eq!(hit_test(100, 2, w, h, 96, 0), Hit::Caption);
        // The band and the edges grow with the scale.
        assert_eq!(hit_test(100, 30, w, h, 192, 3), Hit::Caption);
    }

    #[test]
    fn fr12_a_widget_window_off_every_monitor_comes_back_into_view() {
        let areas = [(0, 0, 1920, 1040)];
        assert_eq!(reachable_origin(100, 100, 200, 100, &areas), (100, 100));
        assert_eq!(reachable_origin(-150, 100, 200, 100, &areas), (-150, 100));
        assert_eq!(reachable_origin(3000, 100, 200, 100, &areas), (1720, 100));
        assert_eq!(reachable_origin(100, -500, 200, 100, &areas), (100, 0));
        assert_eq!(reachable_origin(5, 5, 10, 10, &[]), (5, 5));
    }

    #[test]
    fn fr12_a_widget_window_sits_right_above_the_desktop_not_under_the_wallpaper() {
        // The bottom of the z-order is under the shell's wallpaper window once it has one.
        assert_eq!(z_place(Some(10), Some(20), 30), ZPlace::After(20));
        assert_eq!(z_place(Some(10), Some(30), 30), ZPlace::Keep);
        assert_eq!(z_place(Some(10), None, 30), ZPlace::Keep);
        assert_eq!(z_place(None, None, 30), ZPlace::Bottom);
    }

    #[test]
    fn fr12_a_widget_window_without_an_image_is_logged_once_after_a_while() {
        let start = Instant::now();
        assert!(!frame_overdue(Some(start), false, start));
        assert!(!frame_overdue(
            Some(start),
            false,
            start + NO_FRAME - Duration::from_millis(1)
        ));
        assert!(frame_overdue(Some(start), false, start + NO_FRAME));
        assert!(!frame_overdue(Some(start), true, start + NO_FRAME * 3));
        assert!(!frame_overdue(None, false, start + NO_FRAME * 3));
    }

    #[test]
    fn fr12_a_point_in_the_window_is_scaled_to_the_widgets_frame() {
        assert_eq!(to_frame(50, 25, (100, 50), (200, 100)), (100.0, 50.0));
        assert_eq!(to_frame(500, -3, (100, 50), (200, 100)), (200.0, 0.0));
        assert_eq!(to_frame(7, 8, (0, 0), (200, 100)), (7.0, 8.0));
    }

    fn pointer(action: PointerAction, x: f32) -> WindowReport {
        WindowReport::Pointer {
            widget_id: 1,
            action,
            x,
            y: 0.0,
        }
    }

    #[test]
    fn fr12_moves_waiting_to_be_sent_give_way_to_the_newest() {
        let reports = vec![
            pointer(PointerAction::Down, 1.0),
            pointer(PointerAction::Move, 2.0),
            pointer(PointerAction::Move, 3.0),
            pointer(PointerAction::Move, 4.0),
            pointer(PointerAction::Up, 4.0),
        ];
        assert_eq!(
            coalesce(reports),
            vec![
                pointer(PointerAction::Down, 1.0),
                pointer(PointerAction::Move, 4.0),
                pointer(PointerAction::Up, 4.0),
            ]
        );
    }

    #[test]
    fn fr12_each_change_to_the_widgets_is_one_command_for_the_windows() {
        let widget = Widget {
            widget_id: 3,
            provider: clock(),
            placement: Placement::default(),
        };
        assert_eq!(
            command_of(WidgetEvent::Placed(widget.clone())),
            Some(WindowCommand::Place(widget))
        );
        assert_eq!(
            command_of(WidgetEvent::Removed { widget_id: 3 }),
            Some(WindowCommand::Remove(3))
        );
        assert_eq!(command_of(WidgetEvent::Listed(vec![3])), None);
    }

    #[test]
    fn fr12_a_widget_window_the_shell_put_under_the_desktop_is_put_back_above_it() {
        let (desktop, own, widget) = (10, 1, 2);
        let fits = |list: &[(isize, bool)], ours: &[isize]| {
            let below: Vec<Below> = list
                .iter()
                .map(|&(hwnd, covers)| Below { hwnd, covers })
                .collect();
            right_above_desktop(&below, desktop, ours)
        };
        // Right above the desktop, with a hidden window and another widget between.
        assert!(fits(&[(30, false), (widget, true), (desktop, true)], &[own, widget]));
        // The shell raised its desktop window over it: the desktop is not below it.
        assert!(!fits(&[(40, false), (41, false)], &[own]));
        // An app's window is under it, which it would hide.
        assert!(!fits(&[(50, true), (desktop, true)], &[own]));
    }

    /// A placed widget window shows its placeholder from the moment it opens, through the real
    /// windows: it is given its picture before it is shown, the system takes the picture, the
    /// picture is mostly opaque, and the window is visible, on a monitor and right above the
    /// desktop.
    #[cfg(windows)]
    #[test]
    fn fr12_a_placed_widget_window_shows_its_placeholder_at_once() {
        use windows_sys::Win32::Foundation::{HWND, RECT};
        use windows_sys::Win32::Graphics::Gdi::{MonitorFromRect, MONITOR_DEFAULTTONULL};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindow, GetWindowLongPtrW, GetWindowRect, IsWindowVisible, GWL_EXSTYLE,
            GW_HWNDNEXT, WS_EX_LAYERED,
        };

        let mut windows = WindowsWidgetWindows::open(None).expect("opening the widget windows");
        let placement = Placement {
            x: 40,
            y: 40,
            width_dp: 180,
            height_dp: 110,
        };
        windows.command(WindowCommand::Place(Widget {
            widget_id: 4242,
            provider: clock(),
            placement,
        }));
        let deadline = Instant::now() + Duration::from_secs(10);
        // Only this test opens widget windows, so the first drawing is its window's.
        let (first, hwnd) = loop {
            let found = win::PRESENTED.lock().unwrap().first().copied();
            if let Some(found) = found {
                break (found, found.hwnd as HWND);
            }
            assert!(Instant::now() < deadline, "the widget's window was never drawn");
            std::thread::sleep(Duration::from_millis(20));
        };
        // Shown right after its first drawing.
        std::thread::sleep(Duration::from_millis(500));
        assert!(first.committed, "the system did not take the window's picture");
        assert!(!first.visible, "the window was shown before it had a picture");
        let pixels = (first.width * first.height) as usize;
        assert!(pixels > 0, "the window is {}x{}", first.width, first.height);
        assert!(
            first.opaque * 10 >= pixels * 9,
            "only {} of {pixels} pixels of the placeholder are opaque",
            first.opaque
        );
        // SAFETY: reads a window of the windows' thread.
        unsafe {
            assert!(IsWindowVisible(hwnd) != 0, "the window is hidden");
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            assert!(ex_style & WS_EX_LAYERED != 0);
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetWindowRect(hwnd, &mut rect);
            assert_eq!(
                (rect.right - rect.left, rect.bottom - rect.top),
                (first.width, first.height)
            );
            assert!(
                !MonitorFromRect(&rect, MONITOR_DEFAULTTONULL).is_null(),
                "the window at {},{} is on no monitor",
                rect.left,
                rect.top
            );
            // With a shell, the desktop is below the window, so the desktop is not drawn over it.
            if let Some(desktop) = win::desktop_window() {
                let mut next = GetWindow(hwnd, GW_HWNDNEXT);
                let mut steps = 0;
                while !next.is_null() && next != desktop && steps < 4096 {
                    next = GetWindow(next, GW_HWNDNEXT);
                    steps += 1;
                }
                assert_eq!(next, desktop, "the desktop is drawn over the window");
            }
        }
        windows.command(WindowCommand::Remove(4242));
    }
}
