//! Picture-in-picture windows through XWayland, for desktops whose Wayland side offers no way
//! to keep a window above the others or place it (GNOME).
//!
//! Each task in picture-in-picture gets an X11 window with `_NET_WM_STATE_ABOVE`, placed in
//! the bottom right corner of the screen. Frames read back go in with `PutImage`; frames on the
//! GPU go in without a copy through the CPU: each dmabuf becomes a pixmap (DRI3), and Render
//! composites it onto the window, which also puts its R, G, B order into the window's. The
//! window's pointer input goes to the task as from its Wayland window.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use dsa_host_core::compositor::window_hosts::FromHost;
use dsa_host_core::input::{MouseButton, WindowInput};
use x11rb::connection::Connection;
use x11rb::protocol::dri3::ConnectionExt as _;
use x11rb::protocol::render::{ConnectionExt as _, PictOp};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, ImageFormat, PropMode, WindowClass,
};
use x11rb::protocol::Event as XEvent;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::frames::{Frame, GpuFrame, GpuImage};
use super::wayland::{Event, Events};

/// The margin from the screen's corner, in pixels.
const MARGIN: i32 = 16;

struct PipWindow {
    window: u32,
    gc: u32,
    picture: u32,
}

/// The X11 connection that holds the picture-in-picture windows.
pub struct X11Pip {
    connection: Arc<RustConnection>,
    root: u32,
    root_depth: u8,
    root_visual: u32,
    screen_size: (u16, u16),
    above: [u32; 2],
    delete_window: u32,
    windows: HashMap<i32, PipWindow>,
    by_window: Arc<Mutex<HashMap<u32, i32>>>,
    /// Each display's GPU images as pixmaps with a Render picture, by display and slot.
    images: HashMap<(u32, u32), (u32, u32)>,
    /// The Render format of `R, G, B, A` bytes (the renderer's dmabufs).
    abgr_format: Option<u32>,
    /// The Render format of the windows' visual.
    window_format: Option<u32>,
    dri3: bool,
}

fn atom(connection: &RustConnection, name: &[u8]) -> Option<u32> {
    Some(connection.intern_atom(false, name).ok()?.reply().ok()?.atom)
}

impl X11Pip {
    /// Connects to the X server `DISPLAY` names (XWayland), and reads its pointer input on a
    /// thread of its own. `None` when there is no X server.
    pub fn connect(events: Events) -> Option<X11Pip> {
        std::env::var_os("DISPLAY")?;
        let (connection, screen) = x11rb::connect(None).ok()?;
        let connection = Arc::new(connection);
        let screen = &connection.setup().roots[screen];
        let (root, root_depth, root_visual) = (screen.root, screen.root_depth, screen.root_visual);
        let screen_size = (screen.width_in_pixels, screen.height_in_pixels);
        let state = atom(&connection, b"_NET_WM_STATE")?;
        let above = atom(&connection, b"_NET_WM_STATE_ABOVE")?;
        let delete_window = atom(&connection, b"WM_DELETE_WINDOW")?;
        let dri3 = connection
            .dri3_query_version(1, 2)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        let (abgr_format, window_format) = connection
            .render_query_pict_formats()
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|formats| {
                let abgr = formats
                    .formats
                    .iter()
                    .find(|f| {
                        f.depth == 32
                            && f.direct.red_shift == 0
                            && f.direct.green_shift == 8
                            && f.direct.blue_shift == 16
                            && f.direct.alpha_mask == 0xff
                            && f.direct.alpha_shift == 24
                    })
                    .map(|f| f.id);
                let window = formats
                    .screens
                    .iter()
                    .flat_map(|screen| &screen.depths)
                    .flat_map(|depth| &depth.visuals)
                    .find(|visual| visual.visual == root_visual)
                    .map(|visual| visual.format);
                (abgr, window)
            })
            .unwrap_or((None, None));
        let by_window = Arc::new(Mutex::new(HashMap::new()));
        {
            let connection = connection.clone();
            let by_window = by_window.clone();
            std::thread::spawn(move || {
                read_events(&connection, &by_window, delete_window, &events)
            });
        }
        Some(X11Pip {
            connection,
            root,
            root_depth,
            root_visual,
            screen_size,
            above: [state, above],
            delete_window,
            windows: HashMap::new(),
            by_window,
            images: HashMap::new(),
            abgr_format,
            window_format,
            dri3,
        })
    }

    /// Opens the task's picture-in-picture window, `size` pixels, in the screen's bottom
    /// right corner, above the other windows.
    pub fn open(&mut self, task_id: i32, size: (u32, u32), title: &str) -> bool {
        if self.windows.contains_key(&task_id) {
            return true;
        }
        let c = &*self.connection;
        let (Ok(window), Ok(gc), Ok(picture)) = (c.generate_id(), c.generate_id(), c.generate_id())
        else {
            return false;
        };
        let width = size.0.clamp(1, u32::from(u16::MAX)) as u16;
        let height = size.1.clamp(1, u32::from(u16::MAX)) as u16;
        let x = (i32::from(self.screen_size.0) - i32::from(width) - MARGIN).max(0) as i16;
        let y = (i32::from(self.screen_size.1) - i32::from(height) - MARGIN).max(0) as i16;
        let mask = EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::LEAVE_WINDOW
            | EventMask::STRUCTURE_NOTIFY;
        let created = c.create_window(
            self.root_depth,
            window,
            self.root,
            x,
            y,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            self.root_visual,
            &CreateWindowAux::new().event_mask(mask).background_pixel(0),
        );
        if created.is_err() {
            return false;
        }
        let _ = c.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        );
        let _ = c.change_property32(
            PropMode::REPLACE,
            window,
            self.above[0],
            AtomEnum::ATOM,
            &[self.above[1]],
        );
        if let Some(protocols) = atom(c, b"WM_PROTOCOLS") {
            let _ = c.change_property32(
                PropMode::REPLACE,
                window,
                protocols,
                AtomEnum::ATOM,
                &[self.delete_window],
            );
        }
        let _ = c.create_gc(gc, window, &CreateGCAux::new());
        if let Some(format) = self.window_format {
            let _ = c.render_create_picture(picture, window, format, &Default::default());
        }
        let _ = c.map_window(window);
        // Some window managers place a new window themselves; it goes to the corner again.
        let _ = c.configure_window(
            window,
            &ConfigureWindowAux::new().x(i32::from(x)).y(i32::from(y)),
        );
        // Ask the window manager to keep it above, for one that reads the state only then.
        let _ = c.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(32, window, self.above[0], [1, self.above[1], 0, 1, 0]),
        );
        let _ = c.flush();
        if let Ok(mut by_window) = self.by_window.lock() {
            by_window.insert(window, task_id);
        }
        self.windows.insert(
            task_id,
            PipWindow {
                window,
                gc,
                picture,
            },
        );
        true
    }

    pub fn close(&mut self, task_id: i32) {
        let Some(pip) = self.windows.remove(&task_id) else {
            return;
        };
        if let Ok(mut by_window) = self.by_window.lock() {
            by_window.remove(&pip.window);
        }
        let c = &*self.connection;
        if self.window_format.is_some() {
            let _ = c.render_free_picture(pip.picture);
        }
        let _ = c.free_gc(pip.gc);
        let _ = c.destroy_window(pip.window);
        let _ = c.flush();
    }

    fn fit(&self, pip: &PipWindow, size: (u32, u32)) {
        let _ = self.connection.configure_window(
            pip.window,
            &ConfigureWindowAux::new().width(size.0).height(size.1),
        );
    }

    /// Shows a frame read back to memory (`XRGB8888`, which is a 24-bit Z pixmap's layout).
    pub fn show_pixels(&mut self, task_id: i32, frame: &Frame) {
        let Some(pip) = self.windows.get(&task_id) else {
            return;
        };
        self.fit(pip, (frame.width, frame.height));
        let _ = self.connection.put_image(
            ImageFormat::Z_PIXMAP,
            pip.window,
            pip.gc,
            frame.width as u16,
            frame.height as u16,
            0,
            0,
            0,
            24,
            &frame.pixels,
        );
        let _ = self.connection.flush();
    }

    /// Takes a GPU image of a display as a pixmap, for the frames that will be in it.
    pub fn import(&mut self, image: &GpuImage, fd: OwnedFd) {
        let (true, Some(format)) = (self.dri3, self.abgr_format) else {
            return;
        };
        let c = &*self.connection;
        let (Ok(pixmap), Ok(picture)) = (c.generate_id(), c.generate_id()) else {
            return;
        };
        let size = image.stride * image.height;
        let imported = c.dri3_pixmap_from_buffer(
            pixmap,
            self.root,
            size,
            image.width as u16,
            image.height as u16,
            image.stride as u16,
            32,
            32,
            fd,
        );
        if imported.is_err() {
            return;
        }
        let _ = c.render_create_picture(picture, pixmap, format, &Default::default());
        if let Some((old_pixmap, old_picture)) = self
            .images
            .insert((image.display_id, image.slot), (pixmap, picture))
        {
            let _ = c.render_free_picture(old_picture);
            let _ = c.free_pixmap(old_pixmap);
        }
    }

    /// Shows a frame in one of a display's GPU images.
    pub fn show_gpu(&mut self, task_id: i32, display_id: u32, frame: GpuFrame) {
        let Some(pip) = self.windows.get(&task_id) else {
            return;
        };
        let (Some((_, source)), Some(_)) = (
            self.images.get(&(display_id, frame.slot)),
            self.window_format,
        ) else {
            return;
        };
        self.fit(pip, (frame.width, frame.height));
        let _ = self.connection.render_composite(
            PictOp::SRC,
            *source,
            x11rb::NONE,
            pip.picture,
            0,
            0,
            0,
            0,
            0,
            0,
            frame.width as u16,
            frame.height as u16,
        );
        let _ = self.connection.flush();
    }
}

fn read_events(
    connection: &RustConnection,
    by_window: &Mutex<HashMap<u32, i32>>,
    delete_window: u32,
    events: &Events,
) {
    let task_of = |window: u32| {
        by_window
            .lock()
            .ok()
            .and_then(|map| map.get(&window).copied())
    };
    let button = |detail: u8| match detail {
        1 => Some(MouseButton::Left),
        2 => Some(MouseButton::Middle),
        3 => Some(MouseButton::Right),
        8 => Some(MouseButton::Back),
        9 => Some(MouseButton::Forward),
        _ => None,
    };
    while let Ok(event) = connection.wait_for_event() {
        let (task_id, input) = match event {
            XEvent::ButtonPress(e) | XEvent::ButtonRelease(e) => {
                let Some(task_id) = task_of(e.event) else {
                    continue;
                };
                let down = matches!(event, XEvent::ButtonPress(_));
                let position = (f64::from(e.event_x), f64::from(e.event_y));
                let input = match e.detail {
                    4..=7 if down => {
                        let notches = match e.detail {
                            4 => (0.0, 1.0),
                            5 => (0.0, -1.0),
                            6 => (-1.0, 0.0),
                            _ => (1.0, 0.0),
                        };
                        WindowInput::Wheel { position, notches }
                    }
                    detail => match button(detail) {
                        Some(button) => WindowInput::MouseButton {
                            button,
                            down,
                            position,
                        },
                        None => continue,
                    },
                };
                (task_id, input)
            }
            XEvent::MotionNotify(e) => {
                let Some(task_id) = task_of(e.event) else {
                    continue;
                };
                let position = (f64::from(e.event_x), f64::from(e.event_y));
                (task_id, WindowInput::MouseMove { position })
            }
            XEvent::LeaveNotify(e) => {
                let Some(task_id) = task_of(e.event) else {
                    continue;
                };
                (task_id, WindowInput::MouseLeave)
            }
            XEvent::ClientMessage(e) if e.data.as_data32()[0] == delete_window => {
                if let Some(task_id) = task_of(e.window) {
                    events.send(Event::Report(FromHost::Closed { task_id }));
                }
                continue;
            }
            _ => continue,
        };
        events.send(Event::PipInput { task_id, input });
    }
}
