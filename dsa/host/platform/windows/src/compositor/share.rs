//! "Share on PC": what apps in the user's Android share, in Windows' own share window.
//!
//! The window manager watches the shares of the user's Android over the local API
//! ([`dsa_host_core::share::serve`]). For each one it copies the shared files into the share
//! folder ([`crate::paths::UserRoot::share_spool`]), then opens Windows' share window
//! ([`WindowsShareUi`]) with the same text, link and files, so the user can send them to an app,
//! to a contact, or to a nearby PC with Nearby sharing. The share window belongs to a small
//! window of the window manager's own, which a desktop app needs to show it
//! (`IDataTransferManagerInterop`), placed in the middle of the screen the pointer is on and
//! never seen. Whether the user shared or closed the window goes back to Android.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use dsa_host_core::share::{self, ShareContent, ShareOutcome, ShareUi};
use dsa_transport::Connector;

use crate::compositor::log::{note, CompositorLogger};

/// What the window manager calls itself to the service.
const BUILD: &str = "dsa-wm share";

/// How long the share window may stay open before the share counts as closed.
const SHARE_WINDOW_LIMIT: Duration = Duration::from_secs(30 * 60);

/// Shows what the user's Android shares until `stop`.
pub fn serve(
    mut connector: Box<dyn Connector>,
    stop: Arc<AtomicBool>,
    logger: Option<Arc<CompositorLogger>>,
) {
    let Some(spool) = crate::paths::UserRoot::current().map(|root| root.share_spool()) else {
        note(
            logger.as_deref(),
            "share: the user's folder is not known, so nothing can be shared with the PC",
        );
        return;
    };
    let log = |line: &str| note(logger.as_deref(), line);
    let mut ui = WindowsShareUi;
    share::serve(&mut *connector, BUILD, &mut ui, &spool, &stop, &log);
}

/// Windows' share window, which includes Nearby sharing.
pub struct WindowsShareUi;

impl ShareUi for WindowsShareUi {
    fn share(&mut self, content: &ShareContent) -> ShareOutcome {
        // The share window needs a window of a thread in a single-threaded apartment with a
        // message loop, so each share runs on a thread of its own.
        let content = content.clone();
        let (done, outcome) = mpsc::channel();
        let started = thread::Builder::new()
            .name("dsa-wm-share-window".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(|| show(&content))
                    .unwrap_or_else(|_| Err("the share window failed".into()));
                let _ = done.send(result);
            });
        if let Err(e) = started {
            return ShareOutcome::Failed(format!("the share window could not start: {e}"));
        }
        match outcome.recv() {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(why)) => ShareOutcome::Failed(why),
            Err(_) => ShareOutcome::Failed("the share window failed".into()),
        }
    }
}

/// Whether every file of `content` is still there to share.
fn files_present(content: &ShareContent) -> Result<(), String> {
    for file in &content.files {
        if !Path::new(file).is_file() {
            return Err(format!("{} is gone", file.display()));
        }
    }
    Ok(())
}

/// Shows the share window for `content` and waits until the user shares or closes it.
fn show(content: &ShareContent) -> Result<ShareOutcome, String> {
    files_present(content)?;
    window::show(content, SHARE_WINDOW_LIMIT)
}

mod window {
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use dsa_host_core::share::{ShareContent, ShareOutcome};
    use windows::core::{Interface, HSTRING};
    use windows::ApplicationModel::DataTransfer::{
        DataPackage, DataRequestedEventArgs, DataTransferManager, ShareCompletedEventArgs,
    };
    use windows::Foundation::{TypedEventHandler, Uri};
    use windows::Storage::{IStorageItem, StorageFile};
    use windows::Win32::UI::Shell::IDataTransferManagerInterop;
    use windows_collections::IIterable;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
        GetMessageW, KillTimer, PostMessageW, PostQuitMessage, RegisterClassExW,
        SetForegroundWindow, SetLayeredWindowAttributes, SetTimer, ShowWindow, TranslateMessage,
        LWA_ALPHA, MSG, SW_SHOWNA, WM_APP, WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSEXW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    /// Posted to the window when the share window has finished.
    const WM_SHARE_DONE: u32 = WM_APP + 1;
    /// The timer that ends a share window left open too long.
    const LIMIT_TIMER: usize = 1;

    /// Not decided yet; shared; closed without sharing.
    const PENDING: u8 = 0;
    const SHARED: u8 = 1;
    const CANCELED: u8 = 2;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn fail(what: &str, error: windows::core::Error) -> String {
        format!("{what}: {error}")
    }

    unsafe extern "system" fn anchor_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_SHARE_DONE | WM_TIMER => {
                // SAFETY: this thread's own window.
                unsafe { DestroyWindow(hwnd) };
                0
            }
            WM_CLOSE => {
                // SAFETY: as above.
                unsafe { DestroyWindow(hwnd) };
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

    /// A window that cannot be seen, in the middle of the screen the pointer is on, for the
    /// share window to belong to.
    fn open_anchor() -> Result<HWND, String> {
        let class = wide(&format!("DsaWmShare_{}", std::process::id()));
        // SAFETY: the module handle of this process.
        let instance = unsafe { GetModuleHandleW(null()) };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(anchor_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
            hIconSm: null_mut(),
        };
        // SAFETY: the class name outlives the class, which lives as long as the process; a
        // second registration of the same class fails harmlessly.
        unsafe { RegisterClassExW(&wc) };
        let mut pointer = POINT { x: 0, y: 0 };
        // SAFETY: plain reads of where the pointer is and the screen it is on.
        let (x, y) = unsafe {
            GetCursorPos(&mut pointer);
            let monitor = MonitorFromPoint(pointer, MONITOR_DEFAULTTOPRIMARY);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) != 0 {
                let work = info.rcWork;
                ((work.left + work.right) / 2, (work.top + work.bottom) / 2)
            } else {
                (pointer.x, pointer.y)
            }
        };
        let title = wide("Share on PC");
        // SAFETY: a window of the class registered above.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
                1,
                1,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        if hwnd.is_null() {
            return Err(format!(
                "could not open the share window's owner: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: this thread's own window: fully transparent, shown without taking focus,
        // then brought to the front so the share window opens in front.
        unsafe {
            SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA);
            ShowWindow(hwnd, SW_SHOWNA);
            SetForegroundWindow(hwnd);
        }
        Ok(hwnd)
    }

    /// The shared files, as Windows storage items, for the share window's handler.
    struct Files(Vec<IStorageItem>);

    // SAFETY: a StorageFile is an agile Windows Runtime object, which any thread may call; the
    // share window's handler runs on this thread's message loop in any case.
    unsafe impl Send for Files {}

    impl Files {
        /// The items. A method rather than the field, so a closure takes the whole `Files`,
        /// which may be sent, and not the field alone, which may not.
        fn items(&self) -> &[IStorageItem] {
            &self.0
        }
    }

    /// The shared files as Windows storage items.
    fn storage_items(content: &ShareContent) -> Result<Files, String> {
        let mut items = Vec::new();
        for path in &content.files {
            let path = HSTRING::from(path.as_os_str());
            let file = StorageFile::GetFileFromPathAsync(&path)
                .and_then(|asked| asked.join())
                .map_err(|e| fail("opening a shared file", e))?;
            items.push(
                file.cast::<IStorageItem>()
                    .map_err(|e| fail("a shared file", e))?,
            );
        }
        Ok(Files(items))
    }

    /// Fills the share window's data with `content`.
    fn fill(
        data: &DataPackage,
        content: &ShareContent,
        files: &[IStorageItem],
    ) -> windows::core::Result<()> {
        let properties = data.Properties()?;
        properties.SetTitle(&HSTRING::from(content.title.as_str()))?;
        properties.SetApplicationName(&HSTRING::from("Android"))?;
        let mut text = content.text.clone().unwrap_or_default();
        if let Some(first) = content.links.first() {
            if let Ok(uri) = Uri::CreateUri(&HSTRING::from(first.as_str())) {
                data.SetWebLink(&uri)?;
            }
            // Every link is in the text too, for the apps that take text only.
            for link in &content.links {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(link);
            }
        }
        if !text.is_empty() {
            data.SetText(&HSTRING::from(text.as_str()))?;
        }
        if !files.is_empty() {
            let items: Vec<Option<IStorageItem>> = files.iter().cloned().map(Some).collect();
            let items: IIterable<IStorageItem> = items.into();
            data.SetStorageItemsReadOnly(&items)?;
        }
        Ok(())
    }

    /// Shows the share window for `content`, and waits until the user shares, closes it, or
    /// `limit` passes.
    pub fn show(content: &ShareContent, limit: Duration) -> Result<ShareOutcome, String> {
        let _com = crate::start_menu::ComApartment::enter();
        let files = storage_items(content)?;
        let hwnd = open_anchor()?;
        let anchor = Arc::new(AtomicIsize::new(hwnd as isize));
        let decided = Arc::new(AtomicU8::new(PENDING));
        let result = (|| -> Result<(), String> {
            let interop =
                windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()
                    .map_err(|e| fail("Windows has no share window", e))?;
            let target = windows::Win32::Foundation::HWND(hwnd);
            // SAFETY: a top-level window of this thread.
            let manager: DataTransferManager = unsafe { interop.GetForWindow(target) }
                .map_err(|e| fail("the share window of the window manager", e))?;
            let content = content.clone();
            let (anchor_done, decided_done) = (Arc::clone(&anchor), Arc::clone(&decided));
            let handler = TypedEventHandler::<DataTransferManager, DataRequestedEventArgs>::new(
                move |_, args| {
                    let args = args.ok()?;
                    let data = args.Request()?.Data()?;
                    fill(&data, &content, files.items())?;
                    let (anchor, decided) = (Arc::clone(&anchor_done), Arc::clone(&decided_done));
                    data.ShareCompleted(
                        &TypedEventHandler::<DataPackage, ShareCompletedEventArgs>::new(
                            move |_, _| {
                                finish(&anchor, &decided, SHARED);
                                Ok(())
                            },
                        ),
                    )?;
                    let (anchor, decided) = (Arc::clone(&anchor_done), Arc::clone(&decided_done));
                    data.ShareCanceled(&TypedEventHandler::<
                        DataPackage,
                        windows::core::IInspectable,
                    >::new(move |_, _| {
                        finish(&anchor, &decided, CANCELED);
                        Ok(())
                    }))?;
                    Ok(())
                },
            );
            let token = manager
                .DataRequested(&handler)
                .map_err(|e| fail("filling the share window", e))?;
            // SAFETY: as above.
            let shown = unsafe { interop.ShowShareUIForWindow(target) }
                .map_err(|e| fail("opening the share window", e));
            if shown.is_err() {
                let _ = manager.RemoveDataRequested(token);
                return shown;
            }
            let millis = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
            // SAFETY: a timer of this thread's own window, and its message loop until the
            // window is gone.
            unsafe {
                SetTimer(hwnd, LIMIT_TIMER, millis, None);
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                KillTimer(hwnd, LIMIT_TIMER);
            }
            let _ = manager.RemoveDataRequested(token);
            Ok(())
        })();
        anchor.store(0, Ordering::SeqCst);
        if let Err(why) = result {
            // SAFETY: this thread's own window, if it is still there.
            unsafe { DestroyWindow(hwnd) };
            return Err(why);
        }
        Ok(match decided.load(Ordering::SeqCst) {
            SHARED => ShareOutcome::Shared,
            _ => ShareOutcome::Canceled,
        })
    }

    /// Records how the share window ended, once, and has the window's thread stop waiting.
    fn finish(anchor: &AtomicIsize, decided: &AtomicU8, outcome: u8) {
        let _ = decided.compare_exchange(PENDING, outcome, Ordering::SeqCst, Ordering::SeqCst);
        let hwnd = anchor.load(Ordering::SeqCst);
        if hwnd != 0 {
            // SAFETY: posting to a window handle; a stale one fails harmlessly.
            unsafe { PostMessageW(hwnd as HWND, WM_SHARE_DONE, 0, 0) };
        }
    }
}
