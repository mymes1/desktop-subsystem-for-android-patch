//! How a task window is shown: as an ordinary window, filling its monitor, or as a small
//! picture-in-picture window above the others.
//!
//! The app decides first: a task whose app asked to fill the screen gets a window that fills
//! its monitor, and a task in picture-in-picture a small window that stays on top. The user's
//! F11 then turns full screen on or off over what the app asked for, until the app asks for
//! something else. [`ModeState`] holds one window's side of this; the window host carries out
//! the mode it returns. It names no window system.
//!
//! A window that changes mode still changes size only together with a frame of the new size:
//! the size of the monitor (or of the small window) is proposed like any other, the guest is
//! asked for it, and the window takes it when the app has drawn a frame of it.

/// How a task's window is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum WindowMode {
    /// An ordinary window with a title bar.
    #[default]
    Normal,
    /// Covers its whole monitor, with no title bar.
    Fullscreen,
    /// A small window that stays above the others.
    PictureInPicture,
}

impl WindowMode {
    /// The mode a task's app asks for.
    pub fn of(fullscreen: bool, picture_in_picture: bool) -> WindowMode {
        if picture_in_picture {
            WindowMode::PictureInPicture
        } else if fullscreen {
            WindowMode::Fullscreen
        } else {
            WindowMode::Normal
        }
    }
}

/// One window's mode: what the app asked for, and the user's F11 over it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeState {
    app: WindowMode,
    /// What the user's last F11 chose (`true` for full screen) since the app last asked.
    user_fullscreen: Option<bool>,
}

impl ModeState {
    pub fn new() -> ModeState {
        ModeState::default()
    }

    /// The mode the window is shown in.
    pub fn effective(&self) -> WindowMode {
        match self.user_fullscreen {
            Some(true) => WindowMode::Fullscreen,
            Some(false) if self.app == WindowMode::Fullscreen => WindowMode::Normal,
            Some(false) | None => self.app,
        }
    }

    /// The app asks for `mode`. A new request ends what the user chose with F11; the same
    /// request again leaves it. Returns the mode the window is shown in now.
    pub fn app_asks(&mut self, mode: WindowMode) -> WindowMode {
        if mode != self.app {
            self.app = mode;
            self.user_fullscreen = None;
        }
        self.effective()
    }

    /// The user pressed F11: a window in full screen leaves it, any other fills its monitor.
    /// Returns the mode the window is shown in now.
    pub fn toggle_fullscreen(&mut self) -> WindowMode {
        self.user_fullscreen = Some(self.effective() != WindowMode::Fullscreen);
        self.effective()
    }
}

/// Where a picture-in-picture window of a task `task` pixels in size goes on a work area of
/// `work` (left, top, right, bottom): a quarter of the work area's width at most, a third of
/// its height, the task's shape kept, and `margin` pixels from its bottom right corner. A task
/// smaller than that keeps its size. Returns the client area's left, top, width and height.
pub fn picture_in_picture_place(
    task: (u32, u32),
    work: (i32, i32, i32, i32),
    margin: i32,
) -> (i32, i32, u32, u32) {
    let (left, top, right, bottom) = work;
    let area_width = (right - left).max(1) as u32;
    let area_height = (bottom - top).max(1) as u32;
    let bound = ((area_width / 4).max(1), (area_height / 3).max(1));
    let (width, height) = super::task_windows::fit_within(task, bound);
    let x = (right - margin - width as i32).max(left);
    let y = (bottom - margin - height as i32).max(top);
    (x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr30_an_app_that_fills_the_screen_gets_a_window_that_fills_its_monitor() {
        let mut mode = ModeState::new();
        assert_eq!(mode.effective(), WindowMode::Normal);
        assert_eq!(
            mode.app_asks(WindowMode::of(true, false)),
            WindowMode::Fullscreen
        );
        assert_eq!(
            mode.app_asks(WindowMode::of(false, false)),
            WindowMode::Normal
        );
    }

    #[test]
    fn fr30_f11_turns_full_screen_on_and_off_over_what_the_app_asked() {
        let mut mode = ModeState::new();
        assert_eq!(mode.toggle_fullscreen(), WindowMode::Fullscreen);
        assert_eq!(mode.toggle_fullscreen(), WindowMode::Normal);
        // An app in full screen leaves it for the user's F11, and the same request again does
        // not take it back.
        mode.app_asks(WindowMode::Fullscreen);
        assert_eq!(mode.toggle_fullscreen(), WindowMode::Normal);
        assert_eq!(mode.app_asks(WindowMode::Fullscreen), WindowMode::Normal);
        // A new request from the app ends the user's choice.
        assert_eq!(mode.app_asks(WindowMode::Normal), WindowMode::Normal);
        assert_eq!(
            mode.app_asks(WindowMode::Fullscreen),
            WindowMode::Fullscreen
        );
    }

    #[test]
    fn fr30_picture_in_picture_wins_over_full_screen() {
        assert_eq!(WindowMode::of(true, true), WindowMode::PictureInPicture);
        let mut mode = ModeState::new();
        assert_eq!(
            mode.app_asks(WindowMode::PictureInPicture),
            WindowMode::PictureInPicture
        );
    }

    #[test]
    fn fr30_a_picture_in_picture_window_is_small_and_sits_in_the_bottom_right_corner() {
        let (x, y, width, height) = picture_in_picture_place((1280, 720), (0, 0, 1920, 1040), 16);
        assert_eq!((width, height), (480, 270));
        assert_eq!((x, y), (1920 - 16 - 480, 1040 - 16 - 270));
        // A task smaller than that keeps its size.
        let (_, _, width, height) = picture_in_picture_place((320, 180), (0, 0, 1920, 1040), 16);
        assert_eq!((width, height), (320, 180));
    }
}
