//! The title bar of a task window: transparent, with the app's content running underneath.
//!
//! The window keeps its standard frame, so Windows still gives it its resize borders, its
//! shadow, its rounded corners, its system menu, snapping and the animations of a normal
//! window. Only the caption is taken away: `WM_NCCALCSIZE` makes the client area reach the
//! window's top edge, the app's frames fill it from there, and the window draws its caption
//! buttons over the app's top right corner in a visual of their own
//! ([`WindowPresenter::show_caption`]).
//!
//! The title bar keeps the height Windows gives a caption at the window's DPI. The window
//! reports it (`FromHost::Caption`) when it opens and whenever it changes, and Android gives
//! the task a top inset of that height, so the app's own toolbar sits below the buttons.
//!
//! The band belongs to Windows, as the platform-neutral [`CaptionLayout`] decides:
//! `WM_NCHITTEST` answers a caption button's code over the buttons (the maximize button's
//! code is what shows the Windows 11 snap layouts), the top resize edge's codes along the
//! top, and `HTCAPTION` everywhere else in the band, so dragging moves the window, a double
//! click maximizes it and a right click opens the system menu, all done by Windows. The
//! buttons themselves are pressed here, on the non-client mouse messages, so Windows never
//! draws its classic buttons over them.
//!
//! The glyphs follow the app's status bar (`ToHost::CaptionTone`): dark over a light one,
//! light over a dark one. Under a high contrast theme the buttons take the theme's colors.
//! The title text and the app's icon are not drawn: the taskbar, Alt+Tab and the system menu
//! show them, and the app's own toolbar names what it shows.

use std::cell::Cell;

use dsa_host_core::compositor::caption::{
    paint_buttons, CaptionButton, CaptionHit, CaptionLayout, CaptionPalette, CaptionState,
    CaptionTone,
};
use dsa_host_core::compositor::window_hosts::FromHost;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetSysColor, ScreenToClient, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT,
};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, IsZoomed, PostMessageW, SystemParametersInfoW, HTCAPTION,
    HTCLIENT, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, NCCALCSIZE_PARAMS,
    SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SPI_GETHIGHCONTRAST, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
    WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_SYSCOMMAND,
};

use crate::compositor::present::WindowPresenter;

/// The borders Windows gives a window of `style` at `dpi`: how far its frame reaches past
/// its client area on the left, at the top (the caption with the top border), on the right
/// and at the bottom.
pub unsafe fn standard_frame(
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    dpi: u32,
) -> (i32, i32, i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    AdjustWindowRectExForDpi(&mut rect, style, 0, ex_style, dpi);
    (-rect.left, -rect.top, rect.right, rect.bottom)
}

/// The height of the title bar at `dpi`: the caption and the top border of a standard
/// window, which is where a standard window's client area starts.
pub unsafe fn title_bar_height(style: WINDOW_STYLE, ex_style: WINDOW_EX_STYLE, dpi: u32) -> u32 {
    standard_frame(style, ex_style, dpi).1.max(0) as u32
}

/// The frame of a task window, whose client area reaches its top edge: the standard
/// window's borders without its caption. A maximized window keeps its top border too, which
/// Windows places off the screen, as it does the other three.
pub unsafe fn task_frame(
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    dpi: u32,
    maximized: bool,
) -> (i32, i32, i32, i32) {
    let (left, _, right, bottom) = standard_frame(style, ex_style, dpi);
    (left, if maximized { bottom } else { 0 }, right, bottom)
}

/// Whether a high contrast theme is on.
unsafe fn high_contrast() -> bool {
    let mut contrast: HIGHCONTRASTW = std::mem::zeroed();
    contrast.cbSize = std::mem::size_of::<HIGHCONTRASTW>() as u32;
    SystemParametersInfoW(
        SPI_GETHIGHCONTRAST,
        contrast.cbSize,
        &mut contrast as *mut HIGHCONTRASTW as *mut std::ffi::c_void,
        0,
    ) != 0
        && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
}

/// A system color as 0xAARRGGBB, opaque.
unsafe fn system_color(index: i32) -> u32 {
    let bgr = GetSysColor(index as _);
    0xFF00_0000 | ((bgr & 0xff) << 16) | (bgr & 0xff00) | ((bgr >> 16) & 0xff)
}

/// The colors the buttons are drawn in: the theme's under high contrast, otherwise those of
/// Windows' own caption buttons for glyphs of `tone`.
unsafe fn palette(tone: CaptionTone) -> CaptionPalette {
    if !high_contrast() {
        return CaptionPalette::of(tone);
    }
    let highlight = system_color(COLOR_HIGHLIGHT as i32);
    let highlight_text = system_color(COLOR_HIGHLIGHTTEXT as i32);
    CaptionPalette {
        rest: system_color(COLOR_BTNFACE as i32),
        glyph: system_color(COLOR_BTNTEXT as i32),
        glyph_highlighted: highlight_text,
        glyph_inactive: system_color(COLOR_GRAYTEXT as i32),
        hover: highlight,
        pressed: highlight,
        close_hover: highlight,
        close_pressed: highlight,
        close_glyph: highlight_text,
    }
}

/// The button a hit test code names.
fn button_of(code: u32) -> Option<CaptionButton> {
    match code {
        HTMINBUTTON => Some(CaptionButton::Minimize),
        HTMAXBUTTON => Some(CaptionButton::Maximize),
        HTCLOSE => Some(CaptionButton::Close),
        _ => None,
    }
}

/// The hit test code of what a point belongs to.
fn code_of(hit: CaptionHit) -> u32 {
    match hit {
        CaptionHit::Client => HTCLIENT,
        CaptionHit::Caption => HTCAPTION,
        CaptionHit::Button(CaptionButton::Minimize) => HTMINBUTTON,
        CaptionHit::Button(CaptionButton::Maximize) => HTMAXBUTTON,
        CaptionHit::Button(CaptionButton::Close) => HTCLOSE,
        CaptionHit::Top => HTTOP,
        CaptionHit::TopLeft => HTTOPLEFT,
        CaptionHit::TopRight => HTTOPRIGHT,
    }
}

/// A task window's title bar: the tone and state of its buttons, and the height it last
/// reported.
pub struct CaptionWindow {
    task_id: i32,
    style: WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
    tone: Cell<CaptionTone>,
    hovered: Cell<Option<CaptionButton>>,
    pressed: Cell<Option<CaptionButton>>,
    active: Cell<bool>,
    /// The height last reported; 0 before the first report.
    reported: Cell<u32>,
    /// Whether Windows will say when the pointer leaves the non-client area.
    tracking: Cell<bool>,
    /// What was last drawn, so a message that changes nothing draws nothing.
    drawn: Cell<Option<(CaptionLayout, CaptionState, CaptionPalette, u32)>>,
}

impl CaptionWindow {
    pub fn new(task_id: i32, style: WINDOW_STYLE, ex_style: WINDOW_EX_STYLE) -> CaptionWindow {
        CaptionWindow {
            task_id,
            style,
            ex_style,
            tone: Cell::new(CaptionTone::default()),
            hovered: Cell::new(None),
            pressed: Cell::new(None),
            active: Cell::new(true),
            reported: Cell::new(0),
            tracking: Cell::new(false),
            drawn: Cell::new(None),
        }
    }

    /// Where the title bar and its buttons are in `hwnd` now.
    pub unsafe fn layout(&self, hwnd: HWND) -> CaptionLayout {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        let maximized = IsZoomed(hwnd) != 0;
        let border = if maximized {
            0
        } else {
            standard_frame(self.style, self.ex_style, dpi).3.max(0) as u32
        };
        CaptionLayout::new(
            (client.right - client.left).max(0) as u32,
            title_bar_height(self.style, self.ex_style, dpi),
            dpi,
            border,
        )
    }

    /// Lets DWM draw the window's shadow and border around a client area that reaches the
    /// window's top edge. Called once the window exists.
    pub unsafe fn extend_frame(&self, hwnd: HWND) {
        let margins = MARGINS {
            cxLeftWidth: 0,
            cxRightWidth: 0,
            cyTopHeight: 1,
            cyBottomHeight: 0,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins);
        self.apply_tone(hwnd);
    }

    /// The buttons take `tone`, and the window's frame the matching dark or light mode.
    pub unsafe fn set_tone(
        &self,
        hwnd: HWND,
        tone: CaptionTone,
        presenter: Option<&WindowPresenter>,
    ) {
        if self.tone.replace(tone) != tone {
            self.apply_tone(hwnd);
        }
        self.paint(hwnd, presenter);
    }

    unsafe fn apply_tone(&self, hwnd: HWND) {
        let dark: i32 = i32::from(self.tone.get() == CaptionTone::LightGlyphs);
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }

    /// Reports the title bar's height when it differs from the one last reported.
    pub unsafe fn report_height(&self, hwnd: HWND, report: &dyn Fn(FromHost)) {
        let height = title_bar_height(self.style, self.ex_style, GetDpiForWindow(hwnd).max(96));
        if self.reported.replace(height) != height {
            report(FromHost::Caption {
                task_id: self.task_id,
                height,
            });
        }
    }

    /// Draws the buttons as they are now, unless that is what was drawn last.
    pub unsafe fn paint(&self, hwnd: HWND, presenter: Option<&WindowPresenter>) {
        let Some(presenter) = presenter else {
            return;
        };
        let layout = self.layout(hwnd);
        let dpi = GetDpiForWindow(hwnd).max(96);
        let state = CaptionState {
            hovered: self.hovered.get(),
            pressed: self.pressed.get(),
            maximized: IsZoomed(hwnd) != 0,
            active: self.active.get(),
        };
        let palette = palette(self.tone.get());
        let now = Some((layout, state, palette, dpi));
        if self.drawn.get() == now {
            return;
        }
        let pixels = paint_buttons(&layout, dpi, &palette, &state);
        if presenter
            .show_caption(
                layout.buttons_left(),
                (3 * layout.button_width, layout.height),
                &pixels,
            )
            .is_ok()
        {
            self.drawn.set(now);
        }
    }

    /// Draws the buttons again even if nothing this window knows of changed, as after a
    /// change of theme.
    pub unsafe fn repaint(&self, hwnd: HWND, presenter: Option<&WindowPresenter>) {
        self.drawn.set(None);
        self.paint(hwnd, presenter);
    }

    /// The window became the active one, or stopped being it.
    pub unsafe fn activated(&self, hwnd: HWND, active: bool, presenter: Option<&WindowPresenter>) {
        self.active.set(active);
        self.paint(hwnd, presenter);
    }

    fn hover(&self, button: Option<CaptionButton>) -> bool {
        self.hovered.replace(button) != button
    }

    /// Handles the window's non-client messages: its frame, its hit test and its buttons.
    /// `None` leaves the message to the window's other handlers.
    pub unsafe fn handle(
        &self,
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        presenter: Option<&WindowPresenter>,
    ) -> Option<LRESULT> {
        match message {
            WM_NCCALCSIZE => {
                // The first rectangle is the window's; Windows makes it the client area. Its
                // top is put back, so the client area reaches the window's top edge, except
                // in a maximized window, whose top border is off the screen.
                let rect: *mut RECT = if wparam != 0 {
                    std::ptr::addr_of_mut!((*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0])
                } else {
                    lparam as *mut RECT
                };
                let top = (*rect).top;
                let result = DefWindowProcW(hwnd, message, wparam, lparam);
                let dpi = GetDpiForWindow(hwnd).max(96);
                let maximized = IsZoomed(hwnd) != 0;
                (*rect).top = top + task_frame(self.style, self.ex_style, dpi, maximized).1;
                Some(result)
            }
            WM_NCHITTEST => {
                // The side and bottom borders are Windows' own; the client area holds the
                // title bar.
                let standard = DefWindowProcW(hwnd, message, wparam, lparam);
                if standard as u32 != HTCLIENT {
                    return Some(standard);
                }
                let mut point = POINT {
                    x: (lparam & 0xffff) as i16 as i32,
                    y: ((lparam >> 16) & 0xffff) as i16 as i32,
                };
                ScreenToClient(hwnd, &mut point);
                Some(code_of(self.layout(hwnd).hit(point.x, point.y)) as LRESULT)
            }
            WM_NCMOUSEMOVE => {
                if !self.tracking.get() {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if TrackMouseEvent(&mut track) != 0 {
                        self.tracking.set(true);
                    }
                }
                let button = button_of(wparam as u32);
                if self.hover(button) {
                    self.paint(hwnd, presenter);
                }
                // Over a button, Windows would draw its own; elsewhere it does the rest.
                match button {
                    Some(_) => Some(0),
                    None => Some(DefWindowProcW(hwnd, message, wparam, lparam)),
                }
            }
            WM_NCMOUSELEAVE => {
                self.tracking.set(false);
                let changed = self.hover(None) | self.pressed.replace(None).is_some();
                if changed {
                    self.paint(hwnd, presenter);
                }
                Some(DefWindowProcW(hwnd, message, wparam, lparam))
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => match button_of(wparam as u32) {
                Some(button) => {
                    self.pressed.set(Some(button));
                    self.paint(hwnd, presenter);
                    Some(0)
                }
                None => None,
            },
            WM_NCLBUTTONUP => match button_of(wparam as u32) {
                Some(button) => {
                    let pressed = self.pressed.replace(None);
                    self.paint(hwnd, presenter);
                    if pressed == Some(button) {
                        let command = match button {
                            CaptionButton::Minimize => SC_MINIMIZE,
                            CaptionButton::Maximize if IsZoomed(hwnd) != 0 => SC_RESTORE,
                            CaptionButton::Maximize => SC_MAXIMIZE,
                            CaptionButton::Close => SC_CLOSE,
                        };
                        // Posted, as a click on Windows' own button does, so the window
                        // finishes this message before it changes.
                        PostMessageW(hwnd, WM_SYSCOMMAND, command as WPARAM, 0);
                    }
                    Some(0)
                }
                None => {
                    if self.pressed.replace(None).is_some() {
                        self.paint(hwnd, presenter);
                    }
                    None
                }
            },
            WM_NCACTIVATE => {
                // Windows would paint its caption; there is none. -1 keeps it from painting
                // the frame, which DWM draws.
                self.activated(hwnd, wparam != 0, presenter);
                Some(DefWindowProcW(hwnd, message, wparam, -1))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::WS_OVERLAPPEDWINDOW;

    #[test]
    fn fr22_a_hit_and_its_code_name_the_same_part_of_the_window() {
        for button in CaptionButton::ALL {
            assert_eq!(button_of(code_of(CaptionHit::Button(button))), Some(button));
        }
        assert_eq!(code_of(CaptionHit::Caption), HTCAPTION);
        assert_eq!(code_of(CaptionHit::Client), HTCLIENT);
        assert_eq!(button_of(HTCAPTION), None);
    }

    #[test]
    fn fr22_the_title_bar_is_as_high_as_a_standard_windows_caption_at_every_scale() {
        // SAFETY: AdjustWindowRectExForDpi only computes.
        unsafe {
            for dpi in [96, 144, 192] {
                let height = title_bar_height(WS_OVERLAPPEDWINDOW, 0, dpi);
                let (_, top, _, bottom) = standard_frame(WS_OVERLAPPEDWINDOW, 0, dpi);
                assert_eq!(height as i32, top);
                // A caption is taller than a border, and grows with the scale.
                assert!(top > bottom, "at {dpi}: caption {top}, border {bottom}");
                assert!(
                    height >= 20 * dpi / 96,
                    "at {dpi} the title bar is {height} pixels"
                );
                let (_, task_top, _, task_bottom) =
                    task_frame(WS_OVERLAPPEDWINDOW, 0, dpi, false);
                assert_eq!((task_top, task_bottom), (0, bottom));
                assert_eq!(task_frame(WS_OVERLAPPEDWINDOW, 0, dpi, true).1, bottom);
            }
        }
    }
}
