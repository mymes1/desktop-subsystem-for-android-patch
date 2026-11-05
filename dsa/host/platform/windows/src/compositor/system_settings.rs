//! The user's Windows settings that their Android follows: the time zone, the display
//! languages, the regional format, the app theme and the accent color.
//!
//! The service runs as the machine in session 0, and the languages, the regional format and
//! the theme belong to the user, so the window manager, which runs in the user's session,
//! reads them and gives them to the service ([`dsa_host_core::system_settings::follow`]).
//! A hidden top-level window hears Windows announce a change (`WM_SETTINGCHANGE` for the
//! languages, the regional format and the theme, `WM_TIMECHANGE` for the time zone,
//! `WM_DWMCOLORIZATIONCOLORCHANGED` for the accent color), and the settings are read again
//! then. Windows names its time zones its own way ("Korea Standard Time"); the ICU that ships
//! with Windows gives the IANA name Android uses ("Asia/Seoul").

use std::cell::Cell;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use dsa_host_core::system_settings::{self, SystemSettings, SystemSettingsSource};
use dsa_transport::Connector;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};

use super::log::{note, CompositorLogger};

/// The build the window manager names itself by when it gives the settings.
const BUILD: &str = "dsa-wm system settings";

/// The window class of the hidden window that hears of changes.
const CLASS: &str = "DsaSystemSettings";

/// `WM_DWMCOLORIZATIONCOLORCHANGED`: the accent color changed.
const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;

thread_local! {
    /// Set by the hidden window when Windows announces a change.
    static CHANGED: Cell<bool> = const { Cell::new(false) };
}

/// The accent color as `0xAARRGGBB`, from DWM's `AccentColor`, which is `0xAABBGGRR`. The
/// accent is always opaque.
pub fn accent_from_dwm(value: u32) -> u32 {
    let red = value & 0xff;
    let green = (value >> 8) & 0xff;
    let blue = (value >> 16) & 0xff;
    0xff00_0000 | (red << 16) | (green << 8) | blue
}

/// The first day of the week from `LOCALE_IFIRSTDAYOFWEEK` ("0" is Monday through "6",
/// Sunday): 1 is Monday through 7, Sunday; 0 when Windows said something else.
pub fn first_day_from_windows(value: &str) -> u32 {
    match value.trim().parse::<u32>() {
        Ok(day) if day <= 6 => day + 1,
        _ => 0,
    }
}

/// Whether a time format (`LOCALE_SSHORTTIME`, such as "HH:mm" or "h:mm tt") shows a 24-hour
/// clock: it has an hour in capital H.
pub fn is_24_hour(format: &str) -> bool {
    // Quoted text is literal, not a pattern.
    let mut quoted = false;
    for c in format.chars() {
        match c {
            '\'' => quoted = !quoted,
            'H' if !quoted => return true,
            _ => {}
        }
    }
    false
}

/// The strings in a list of null-terminated UTF-16 strings that ends with an empty one, as
/// `GetUserPreferredUILanguages` returns them.
pub fn multi_string(units: &[u16]) -> Vec<String> {
    units
        .split(|unit| *unit == 0)
        .take_while(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A UTF-16 buffer Windows filled, up to its first null.
fn from_wide(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// A DWORD under the current user's registry key `path`, if it is there.
fn user_dword(path: &str, name: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let (path, name) = (wide(path), wide(name));
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: null-terminated strings and a DWORD with its size, all outliving the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    (status == 0).then_some(value)
}

/// One of the user's locale values (`GetLocaleInfoEx` for the user's default locale).
fn locale_info(kind: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::GetLocaleInfoEx;
    let mut buffer = [0u16; 128];
    // SAFETY: the user's default locale (null) and a buffer of the length passed.
    let length = unsafe {
        GetLocaleInfoEx(
            std::ptr::null(),
            kind,
            buffer.as_mut_ptr(),
            buffer.len() as i32,
        )
    };
    (length > 0).then(|| from_wide(&buffer))
}

/// The user's display languages, preferred first.
fn ui_languages() -> Vec<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let mut count = 0u32;
    let mut length = 0u32;
    // SAFETY: asks for the length only.
    let sized = unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            std::ptr::null_mut(),
            &mut length,
        )
    };
    if sized == 0 || length == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; length as usize];
    // SAFETY: a buffer of the length Windows asked for.
    let read = unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            buffer.as_mut_ptr(),
            &mut length,
        )
    };
    if read == 0 {
        return Vec::new();
    }
    multi_string(&buffer)
}

/// The locale the user formats dates, times and numbers by.
fn regional_format() -> String {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buffer = [0u16; 85];
    // SAFETY: a buffer of the length passed (LOCALE_NAME_MAX_LENGTH).
    let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    if length > 0 {
        from_wide(&buffer)
    } else {
        String::new()
    }
}

/// The IANA name of the computer's time zone, through the ICU that ships with Windows.
fn time_zone() -> String {
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    use windows_sys::Win32::System::Time::{
        GetDynamicTimeZoneInformation, DYNAMIC_TIME_ZONE_INFORMATION,
    };
    // SAFETY: a plain structure Windows fills in.
    let mut info: DYNAMIC_TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid pointer to that structure.
    if unsafe { GetDynamicTimeZoneInformation(&mut info) } == u32::MAX {
        return String::new();
    }
    let key = &info.TimeZoneKeyName;
    let key_length = key.iter().position(|unit| *unit == 0).unwrap_or(key.len());
    if key_length == 0 {
        return String::new();
    }
    type WindowsToIana = unsafe extern "C" fn(
        winid: *const u16,
        length: i32,
        region: *const u8,
        id: *mut u16,
        capacity: i32,
        status: *mut i32,
    ) -> i32;
    let library = wide("icu.dll");
    // SAFETY: a null-terminated name, loaded from System32 only.
    let icu = unsafe {
        LoadLibraryExW(
            library.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if icu.is_null() {
        return String::new();
    }
    // SAFETY: a null-terminated name of a function the module exports.
    let found = unsafe { GetProcAddress(icu, c"ucal_getTimeZoneIDForWindowsID".as_ptr().cast()) };
    let Some(found) = found else {
        return String::new();
    };
    // SAFETY: the function ICU documents under that name has this signature.
    let convert: WindowsToIana = unsafe { std::mem::transmute(found) };
    let mut id = [0u16; 64];
    let mut status = 0i32;
    // SAFETY: the key name and its length, no region, and a buffer of the capacity passed.
    let length = unsafe {
        convert(
            key.as_ptr(),
            key_length as i32,
            std::ptr::null(),
            id.as_mut_ptr(),
            id.len() as i32,
            &mut status,
        )
    };
    // The module stays loaded: it is part of Windows, and loading it again costs a lookup.
    if status > 0 || length <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&id[..(length as usize).min(id.len())])
}

/// Reads the user's settings now.
pub fn read() -> SystemSettings {
    use windows_sys::Win32::Globalization::{LOCALE_IFIRSTDAYOFWEEK, LOCALE_SSHORTTIME};
    let dark_mode = user_dword(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    )
    .is_some_and(|light| light == 0);
    let accent_color = user_dword("Software\\Microsoft\\Windows\\DWM", "AccentColor")
        .map(accent_from_dwm)
        .unwrap_or(0);
    SystemSettings {
        time_zone: time_zone(),
        locales: ui_languages(),
        regional_format: regional_format(),
        time_24_hour: locale_info(LOCALE_SSHORTTIME).is_some_and(|format| is_24_hour(&format)),
        first_day_of_week: locale_info(LOCALE_IFIRSTDAYOFWEEK)
            .map(|day| first_day_from_windows(&day))
            .unwrap_or(0),
        dark_mode,
        accent_color,
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WM_THEMECHANGED, WM_TIMECHANGE,
    };
    if matches!(
        message,
        WM_SETTINGCHANGE
            | WM_TIMECHANGE
            | WM_THEMECHANGED
            | WM_SYSCOLORCHANGE
            | WM_DWMCOLORIZATIONCOLORCHANGED
    ) {
        CHANGED.with(|changed| changed.set(true));
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

/// The user's settings, with the hidden window that hears of their changes. It belongs to
/// the thread that made it, which must also wait on it.
pub struct WindowsSettings {
    window: HWND,
}

impl WindowsSettings {
    /// Opens the hidden window. Without it, changes are still found when the settings are
    /// read again every so often.
    pub fn open() -> WindowsSettings {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, RegisterClassExW, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
        };
        let class = wide(CLASS);
        // SAFETY: this module's handle, a class whose procedure lives as long as the program,
        // and a hidden window of this thread, destroyed on drop.
        let window = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let mut description: WNDCLASSEXW = std::mem::zeroed();
            description.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
            description.lpfnWndProc = Some(window_proc);
            description.hInstance = instance;
            description.lpszClassName = class.as_ptr();
            // A second registration fails harmlessly: the class is the same.
            RegisterClassExW(&description);
            // A top-level window, never shown: message-only windows hear no broadcasts.
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        WindowsSettings { window }
    }
}

impl Drop for WindowsSettings {
    fn drop(&mut self) {
        if !self.window.is_null() {
            // SAFETY: the window this thread made.
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.window) };
        }
    }
}

impl SystemSettingsSource for WindowsSettings {
    fn read(&mut self) -> SystemSettings {
        read()
    }

    fn wait_for_change(&mut self, timeout: Duration) -> bool {
        use windows_sys::Win32::System::Threading::INFINITE;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG,
            PM_REMOVE, QS_ALLINPUT,
        };
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(INFINITE - 1);
        // SAFETY: no handles, only this thread's messages.
        unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, millis, QS_ALLINPUT) };
        // SAFETY: a message structure this thread owns, for its own messages.
        unsafe {
            let mut message: MSG = std::mem::zeroed();
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        CHANGED.with(|changed| changed.replace(false))
    }
}

/// Gives the user's Android their Windows settings until `stop` is set.
pub fn follow(
    mut connector: Box<dyn Connector>,
    stop: Arc<AtomicBool>,
    logger: Option<Arc<CompositorLogger>>,
) {
    let mut source = WindowsSettings::open();
    let log = |line: &str| note(logger.as_deref(), line);
    system_settings::follow(&mut *connector, BUILD, &mut source, &stop, &log);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr30_the_accent_color_is_read_as_rgb() {
        // DWM keeps it as 0xAABBGGRR.
        assert_eq!(accent_from_dwm(0xffd4_7800), 0xff00_78d4);
        assert_eq!(accent_from_dwm(0x0011_2233), 0xff33_2211);
    }

    #[test]
    fn fr30_the_first_day_of_the_week_and_the_clock_follow_the_regional_format() {
        assert_eq!(first_day_from_windows("0"), 1);
        assert_eq!(first_day_from_windows("6"), 7);
        assert_eq!(first_day_from_windows("x"), 0);
        assert!(is_24_hour("HH:mm"));
        assert!(!is_24_hour("h:mm tt"));
        assert!(!is_24_hour("h 'H' mm"));
    }

    #[test]
    fn fr30_the_display_languages_are_read_in_order() {
        let units: Vec<u16> = "ko-KR\0en-US\0\0".encode_utf16().collect();
        assert_eq!(multi_string(&units), vec!["ko-KR", "en-US"]);
        assert!(multi_string(&[0, 0]).is_empty());
    }

    #[test]
    fn fr30_the_settings_of_this_session_read_without_failing() {
        let settings = read().checked();
        assert!(settings.locales.len() <= dsa_host_core::system_settings::MAX_LOCALES);
    }
}
