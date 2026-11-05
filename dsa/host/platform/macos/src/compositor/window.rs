//! One `NSWindow` per Android task.
//!
//! The window's content view is the task's input view ([`crate::input::view`]), which hosts
//! the task's `CAMetalLayer` ([`super::metal`]). The window's delegate reports resizes, the
//! close button, focus and display changes to the code that runs the task, which passes them
//! to the guest. Closing a window asks the guest to finish its task; the window goes when the
//! task does, as on every other host.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::sel;

use super::metal::{FrameRef, Presenter};
use super::sizing::{picture_in_picture, task_pixels, window_points, Rect, MIN_CONTENT_POINTS};
use crate::input::view::{self, InputSink};
use crate::objc::{class, ns_string, CGPoint, CGRect, CGSize};

/// What a task window tells the code running its task.
pub trait WindowEvents {
    /// The window's content is now `pixels` big: the guest's task should take that size.
    fn resized(&mut self, pixels: (u32, u32));
    /// The user asked the window to close: the guest should finish the task.
    fn close_requested(&mut self);
    /// The window gained or lost the keyboard focus.
    fn focused(&mut self, focused: bool);
    /// The window moved to a display of another backing scale.
    fn scale_changed(&mut self, scale: f64);
}

const STYLE_TITLED: usize = 1 << 0;
const STYLE_CLOSABLE: usize = 1 << 1;
const STYLE_MINIATURIZABLE: usize = 1 << 2;
const STYLE_RESIZABLE: usize = 1 << 3;
const BACKING_BUFFERED: usize = 2;
const BEHAVIOR_CAN_JOIN_ALL_SPACES: usize = 1 << 0;
const BEHAVIOR_FULL_SCREEN_PRIMARY: usize = 1 << 7;
const LEVEL_NORMAL: isize = 0;
const LEVEL_FLOATING: isize = 3;

thread_local! {
    /// Each open window's events, by the window's address. Only the main thread touches
    /// windows, so only it has entries.
    static EVENTS: RefCell<HashMap<usize, Box<dyn WindowEvents>>> = RefCell::new(HashMap::new());
}

fn with_events(window: *mut AnyObject, f: impl FnOnce(&mut dyn WindowEvents)) {
    // A callback that panics must not unwind into AppKit.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        EVENTS.with(|events| {
            if let Some(events) = events.borrow_mut().get_mut(&(window as usize)) {
                f(events.as_mut());
            }
        })
    }));
}

unsafe fn content_pixels(window: *mut AnyObject) -> ((u32, u32), f64) {
    let view: *mut AnyObject = msg_send![window, contentView];
    let frame: CGRect = msg_send![view, frame];
    let scale: f64 = msg_send![window, backingScaleFactor];
    (
        task_pixels((frame.size.width, frame.size.height), scale),
        scale,
    )
}

unsafe extern "C-unwind" fn did_resize(_this: &AnyObject, _: Sel, note: *mut AnyObject) {
    let window: *mut AnyObject = msg_send![note, object];
    let (pixels, _) = content_pixels(window);
    with_events(window, |events| events.resized(pixels));
}

unsafe extern "C-unwind" fn should_close(_this: &AnyObject, _: Sel, window: *mut AnyObject) -> Bool {
    with_events(window, |events| events.close_requested());
    // The window closes when the guest's task finishes, not before.
    Bool::NO
}

unsafe extern "C-unwind" fn did_become_key(_this: &AnyObject, _: Sel, note: *mut AnyObject) {
    let window: *mut AnyObject = msg_send![note, object];
    with_events(window, |events| events.focused(true));
}

unsafe extern "C-unwind" fn did_resign_key(_this: &AnyObject, _: Sel, note: *mut AnyObject) {
    let window: *mut AnyObject = msg_send![note, object];
    with_events(window, |events| events.focused(false));
}

unsafe extern "C-unwind" fn did_change_backing(_this: &AnyObject, _: Sel, note: *mut AnyObject) {
    let window: *mut AnyObject = msg_send![note, object];
    let (pixels, scale) = content_pixels(window);
    with_events(window, |events| {
        events.scale_changed(scale);
        events.resized(pixels);
    });
}

fn delegate_class() -> Result<&'static AnyClass, String> {
    static CLASS: OnceLock<Result<&'static AnyClass, String>> = OnceLock::new();
    CLASS
        .get_or_init(|| unsafe {
            let superclass = class(c"NSObject")?;
            let mut builder = ClassBuilder::new(c"DSATaskWindowDelegate", superclass)
                .ok_or("the window delegate class is already defined")?;
            builder.add_method(
                sel!(windowDidResize:),
                did_resize as unsafe extern "C-unwind" fn(_, _, _),
            );
            builder.add_method(
                sel!(windowShouldClose:),
                should_close as unsafe extern "C-unwind" fn(_, _, _) -> _,
            );
            builder.add_method(
                sel!(windowDidBecomeKey:),
                did_become_key as unsafe extern "C-unwind" fn(_, _, _),
            );
            builder.add_method(
                sel!(windowDidResignKey:),
                did_resign_key as unsafe extern "C-unwind" fn(_, _, _),
            );
            builder.add_method(
                sel!(windowDidChangeBackingProperties:),
                did_change_backing as unsafe extern "C-unwind" fn(_, _, _),
            );
            Ok(builder.register())
        })
        .clone()
}

/// One task's window.
pub struct TaskWindow {
    window: Retained<AnyObject>,
    view: Retained<AnyObject>,
    /// The window delegate; the window holds it only weakly.
    _delegate: Retained<AnyObject>,
    presenter: Presenter,
    /// The frame to go back to when picture in picture ends.
    before_pip: Option<CGRect>,
    /// The task's size, for picture in picture to keep its shape.
    task: (u32, u32),
}

impl TaskWindow {
    /// Opens a window titled `title` for task `task_id` showing a task of `pixels`, centered on the main
    /// display. Must be called on the main thread.
    pub fn open(
        task_id: i32,
        title: &str,
        pixels: (u32, u32),
        events: Box<dyn WindowEvents>,
        input: Box<dyn InputSink>,
    ) -> Result<TaskWindow, String> {
        if !crate::objc::is_main_thread() {
            return Err("a task window can only be opened on the main thread".into());
        }
        unsafe {
            let window_class = class(c"NSWindow")?;
            let screen_class = class(c"NSScreen")?;
            let screen: *mut AnyObject = msg_send![screen_class, mainScreen];
            let scale: f64 = if screen.is_null() {
                1.0
            } else {
                msg_send![screen, backingScaleFactor]
            };
            let points = window_points(pixels, scale);
            let rect = CGRect {
                origin: CGPoint::default(),
                size: CGSize {
                    width: points.0.max(MIN_CONTENT_POINTS.0),
                    height: points.1.max(MIN_CONTENT_POINTS.1),
                },
            };
            let allocated: Allocated<AnyObject> = msg_send![window_class, alloc];
            let window: Option<Retained<AnyObject>> = msg_send![
                allocated,
                initWithContentRect: rect,
                styleMask: STYLE_TITLED | STYLE_CLOSABLE | STYLE_MINIATURIZABLE | STYLE_RESIZABLE,
                backing: BACKING_BUFFERED,
                defer: false
            ];
            let window = window.ok_or("AppKit could not make a window")?;
            let _: () = msg_send![&*window, setReleasedWhenClosed: false];
            let title = ns_string(title);
            let _: () = msg_send![&*window, setTitle: &*title];
            let min = CGSize {
                width: MIN_CONTENT_POINTS.0,
                height: MIN_CONTENT_POINTS.1,
            };
            let _: () = msg_send![&*window, setContentMinSize: min];
            let _: () = msg_send![&*window, setCollectionBehavior: BEHAVIOR_FULL_SCREEN_PRIMARY];

            let presenter = Presenter::new(scale)?;
            let view = view::make(task_id, rect, presenter.layer(), input)?;
            let _: () = msg_send![&*window, setContentView: &*view];
            let _: () = msg_send![&*window, makeFirstResponder: &*view];

            let delegate_class = delegate_class()?;
            let delegate: Option<Retained<AnyObject>> = msg_send![delegate_class, new];
            let delegate = delegate.ok_or("no window delegate could be made")?;
            let _: () = msg_send![&*window, setDelegate: &*delegate];
            EVENTS.with(|map| {
                map.borrow_mut()
                    .insert(Retained::as_ptr(&window) as usize, events)
            });

            let _: () = msg_send![&*window, center];
            let _: () = msg_send![&*window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
            Ok(TaskWindow {
                window,
                view,
                _delegate: delegate,
                presenter,
                before_pip: None,
                task: pixels,
            })
        }
    }

    /// The `NSWindow`.
    pub fn ns_window(&self) -> &AnyObject {
        &self.window
    }

    pub fn set_title(&self, title: &str) {
        let title = ns_string(title);
        unsafe {
            let _: () = msg_send![&*self.window, setTitle: &*title];
        }
    }

    /// Shows one frame of the task.
    pub fn present(&mut self, frame: FrameRef<'_>) -> Result<(), String> {
        if frame.content != (0, 0) {
            self.task = frame.content;
        }
        self.presenter.present(frame)
    }

    /// The display's backing scale changed.
    pub fn set_scale(&self, scale: f64) {
        self.presenter.set_scale(scale);
    }

    /// Gives the window's content the size that shows a task of `pixels` one to one, as when
    /// the guest itself resizes its task.
    pub fn resize_to(&mut self, pixels: (u32, u32)) {
        self.task = pixels;
        unsafe {
            let scale: f64 = msg_send![&*self.window, backingScaleFactor];
            let points = window_points(pixels, scale);
            let size = CGSize {
                width: points.0,
                height: points.1,
            };
            let _: () = msg_send![&*self.window, setContentSize: size];
        }
    }

    /// Enters or leaves full screen in its own space.
    pub fn toggle_full_screen(&self) {
        unsafe {
            let _: () = msg_send![&*self.window, toggleFullScreen: std::ptr::null::<AnyObject>()];
        }
    }

    /// Whether the window is in picture in picture.
    pub fn is_picture_in_picture(&self) -> bool {
        self.before_pip.is_some()
    }

    /// Picture in picture: a small window in the screen's corner that floats above the
    /// others on every space, keeping the task's shape; or back to where it was.
    pub fn set_picture_in_picture(&mut self, on: bool) {
        unsafe {
            match (on, self.before_pip) {
                (true, None) => {
                    let frame: CGRect = msg_send![&*self.window, frame];
                    let screen: *mut AnyObject = msg_send![&*self.window, screen];
                    if screen.is_null() {
                        return;
                    }
                    let visible: CGRect = msg_send![screen, visibleFrame];
                    let rect = picture_in_picture(
                        Rect {
                            x: visible.origin.x,
                            y: visible.origin.y,
                            width: visible.size.width,
                            height: visible.size.height,
                        },
                        self.task,
                    );
                    let content = CGRect {
                        origin: CGPoint {
                            x: rect.x,
                            y: rect.y,
                        },
                        size: CGSize {
                            width: rect.width,
                            height: rect.height,
                        },
                    };
                    let target: CGRect = msg_send![&*self.window, frameRectForContentRect: content];
                    let _: () = msg_send![&*self.window, setLevel: LEVEL_FLOATING];
                    let _: () = msg_send![
                        &*self.window,
                        setCollectionBehavior: BEHAVIOR_FULL_SCREEN_PRIMARY | BEHAVIOR_CAN_JOIN_ALL_SPACES
                    ];
                    let aspect = CGSize {
                        width: f64::from(self.task.0.max(1)),
                        height: f64::from(self.task.1.max(1)),
                    };
                    let _: () = msg_send![&*self.window, setContentAspectRatio: aspect];
                    let _: () = msg_send![&*self.window, setFrame: target, display: true, animate: true];
                    self.before_pip = Some(frame);
                }
                (false, Some(frame)) => {
                    let _: () = msg_send![&*self.window, setLevel: LEVEL_NORMAL];
                    let _: () = msg_send![&*self.window, setCollectionBehavior: BEHAVIOR_FULL_SCREEN_PRIMARY];
                    // Resize increments and an aspect ratio exclude each other, so setting
                    // increments of one point lifts the aspect lock.
                    let free = CGSize {
                        width: 1.0,
                        height: 1.0,
                    };
                    let _: () = msg_send![&*self.window, setContentResizeIncrements: free];
                    let _: () = msg_send![&*self.window, setFrame: frame, display: true, animate: true];
                    self.before_pip = None;
                }
                _ => {}
            }
        }
    }

    /// The smallest and largest the window's content may be, in task pixels.
    pub fn set_limits(&self, min: Option<(u32, u32)>, max: Option<(u32, u32)>) {
        unsafe {
            let scale: f64 = msg_send![&*self.window, backingScaleFactor];
            let min = min.map(|m| window_points(m, scale)).unwrap_or((0.0, 0.0));
            let min = CGSize {
                width: min.0.max(MIN_CONTENT_POINTS.0),
                height: min.1.max(MIN_CONTENT_POINTS.1),
            };
            let max = max
                .map(|m| window_points(m, scale))
                .unwrap_or((f64::MAX, f64::MAX));
            let max = CGSize {
                width: max.0.max(min.width),
                height: max.1.max(min.height),
            };
            let _: () = msg_send![&*self.window, setContentMinSize: min];
            let _: () = msg_send![&*self.window, setContentMaxSize: max];
        }
    }

    /// Whether the user may resize the window: an app that asks for a fixed size gets a
    /// window without the resize edges and the zoom button.
    pub fn set_resizable(&self, resizable: bool) {
        unsafe {
            let mask: usize = msg_send![&*self.window, styleMask];
            let mask = if resizable {
                mask | STYLE_RESIZABLE
            } else {
                mask & !STYLE_RESIZABLE
            };
            let _: () = msg_send![&*self.window, setStyleMask: mask];
        }
    }

    /// Whether the window is in full screen now.
    pub fn is_full_screen(&self) -> bool {
        const STYLE_FULL_SCREEN: usize = 1 << 14;
        let mask: usize = unsafe { msg_send![&*self.window, styleMask] };
        mask & STYLE_FULL_SCREEN != 0
    }

    /// Enters or leaves full screen, as the app asks.
    pub fn set_full_screen(&self, on: bool) {
        if self.is_full_screen() != on {
            self.toggle_full_screen();
        }
    }

    /// Brings the window to the front and gives it the keyboard.
    pub fn activate(&self) {
        unsafe {
            let _: () = msg_send![&*self.window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
        }
    }

    /// The input view, for the input method's cursor rectangle and for drag and drop.
    pub fn view(&self) -> &AnyObject {
        &self.view
    }
}

impl Drop for TaskWindow {
    fn drop(&mut self) {
        let window = Retained::as_ptr(&self.window) as usize;
        EVENTS.with(|map| map.borrow_mut().remove(&window));
        view::forget(&self.view);
        unsafe {
            let _: () = msg_send![&*self.window, setDelegate: std::ptr::null::<AnyObject>()];
            let _: () = msg_send![&*self.window, close];
        }
    }
}
