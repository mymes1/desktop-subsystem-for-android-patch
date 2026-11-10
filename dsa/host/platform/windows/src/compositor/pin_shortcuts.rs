//! An Android app's request to put one of its shortcuts on the desktop (`requestPinShortcut`).
//!
//! The agent is Android's default launcher, so the request comes to it, and it passes the
//! request to the host, as a phone's launcher would ask on its home screen. The window manager
//! asks the user, in a Windows task dialog under the app's name with the shortcut's name and
//! icon, [Add to desktop] or [Cancel]. On yes the shortcut goes on the desktop
//! ([`crate::desktop_shortcuts`]), starting that shortcut of the app as its jump list task
//! does, and Android is told to pin it; otherwise Android is told no. It goes when the app is
//! uninstalled, as the app's own desktop shortcut does.
//!
//! The questions are asked one at a time on a thread of their own, so the windows never wait
//! on the user. A question nobody answers within [`ANSWER_WITHIN`] is answered no, as is one
//! that waited that long behind another.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dsa_host_core::host_api::HostApiClient;
use dsa_host_core::tasks::PinShortcutRequest;
use dsa_transport::Connector;

use crate::compositor::log::{note, CompositorLogger};
use crate::desktop_shortcuts::{entry_in, Desktop};
use crate::start_menu::{ShellLinks, Shortcuts};

/// What the window manager calls itself to the service.
const BUILD: &str = "dsa-wm pin shortcuts";

/// How long a request to the service is waited for.
const ANSWER: Duration = Duration::from_secs(5);

/// How long a question waits for the user before it is answered no.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(180);

/// The words of the question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texts {
    korean: bool,
}

impl Texts {
    pub fn english() -> Texts {
        Texts { korean: false }
    }

    pub fn korean() -> Texts {
        Texts { korean: true }
    }

    /// The words in the user's display language.
    pub fn for_user() -> Texts {
        // SAFETY: reads the user's display language.
        let language = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        Texts {
            korean: crate::compositor::widget_menu::is_korean(language),
        }
    }

    /// The question's heading, about the shortcut named `shortcut`.
    pub fn instruction(&self, shortcut: &str) -> String {
        if self.korean {
            format!("바탕 화면에 \u{201c}{shortcut}\u{201d} 바로 가기를 추가할까요?")
        } else {
            format!("Add \u{201c}{shortcut}\u{201d} to the desktop?")
        }
    }

    /// What the question says below its heading, for the app named `app`.
    pub fn content(&self, app: &str) -> String {
        if self.korean {
            format!(
                "{app}에서 이 바로 가기를 바탕 화면에 두려고 합니다. \
                 앱을 지우면 바로 가기도 사라집니다."
            )
        } else {
            format!(
                "{app} wants to put this shortcut on your desktop. It goes when the app is \
                 uninstalled."
            )
        }
    }

    pub fn add(&self) -> &'static str {
        if self.korean {
            "바탕 화면에 추가"
        } else {
            "Add to desktop"
        }
    }

    pub fn cancel(&self) -> &'static str {
        if self.korean {
            "취소"
        } else {
            "Cancel"
        }
    }
}

/// The question as it is put to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The dialog's title: the app's name.
    pub title: String,
    pub instruction: String,
    pub content: String,
    pub add: String,
    pub cancel: String,
    /// The shortcut's icon as a PNG image; empty for none.
    pub icon_png: Vec<u8>,
}

/// The question about `request`.
pub fn question(request: &PinShortcutRequest, texts: &Texts) -> Question {
    let app = if request.app_label.trim().is_empty() {
        request.package_name.clone()
    } else {
        request.app_label.trim().to_string()
    };
    Question {
        instruction: texts.instruction(request.shortcut.label()),
        content: texts.content(&app),
        add: texts.add().to_string(),
        cancel: texts.cancel().to_string(),
        icon_png: request.shortcut.icon_png.clone(),
        title: app,
    }
}

/// Puts a question to the user.
pub trait Ask {
    /// Whether the user said yes to `question` within `within`; no when the time ran out.
    fn ask(&self, question: &Question, within: Duration) -> bool;
}

/// Asks in a Windows task dialog.
pub struct TaskDialogAsk;

impl Ask for TaskDialogAsk {
    fn ask(&self, question: &Question, within: Duration) -> bool {
        dialog::ask(question, within)
    }
}

/// Where a pinned shortcut is made: the user's desktop, and the folder of the apps' programs.
pub struct Place {
    pub desktop: Desktop,
    pub apps_dir: PathBuf,
}

impl Place {
    /// The user's own desktop and apps folder.
    pub fn for_user() -> Option<Place> {
        Some(Place {
            desktop: Desktop::default_for_user()?,
            apps_dir: crate::app_exe::default_apps_dir()?,
        })
    }

    /// Makes the desktop shortcut that starts the shortcut of `request`; returns its path.
    pub fn make(
        &self,
        request: &PinShortcutRequest,
        shortcuts: &dyn Shortcuts,
    ) -> Result<PathBuf, String> {
        let entry = entry_in(&self.apps_dir, &request.package_name, &request.app_label)?;
        let app_dir = self.apps_dir.join(&request.package_name);
        self.desktop
            .place_shortcut(&entry, &request.shortcut, &app_dir, shortcuts)
    }
}

/// Decides one request: asks the user unless it waited too long already, makes the desktop
/// shortcut on yes, and returns the answer for Android (yes only when the shortcut was made)
/// and what to log.
pub fn decide(
    request: &PinShortcutRequest,
    waited: Duration,
    ask: &dyn Ask,
    texts: &Texts,
    place: Option<&Place>,
    shortcuts: &dyn Shortcuts,
) -> (bool, String) {
    let what = format!(
        "{}'s shortcut {:?}",
        request.package_name,
        request.shortcut.label()
    );
    let Some(within) = ANSWER_WITHIN
        .checked_sub(waited)
        .filter(|left| !left.is_zero())
    else {
        return (
            false,
            format!("{what} waited too long to be asked; declined"),
        );
    };
    let Some(place) = place else {
        return (
            false,
            format!("{what}: the desktop cannot be found; declined"),
        );
    };
    if !ask.ask(&question(request, texts), within) {
        return (false, format!("{what}: the user declined"));
    }
    match place.make(request, shortcuts) {
        Ok(path) => (
            true,
            format!("{what} is on the desktop: {}", path.display()),
        ),
        Err(why) => (false, format!("{what} could not go on the desktop: {why}")),
    }
}

/// Asks the user about each request that arrives on `requests`, until it closes, and gives
/// Android the answers through the service on `connector`.
pub fn serve(
    requests: Receiver<(PinShortcutRequest, Instant)>,
    mut connector: Box<dyn Connector>,
    logger: Option<Arc<CompositorLogger>>,
) {
    let _com = crate::start_menu::ComApartment::enter();
    let texts = Texts::for_user();
    let place = Place::for_user();
    while let Ok((request, arrived)) = requests.recv() {
        let (accepted, said) = decide(
            &request,
            arrived.elapsed(),
            &TaskDialogAsk,
            &texts,
            place.as_ref(),
            &ShellLinks,
        );
        note(logger.as_deref(), &format!("pin shortcut: {said}"));
        let answered = HostApiClient::connect(&mut *connector, BUILD, ANSWER)
            .and_then(|mut client| client.answer_pin_shortcut(request.request_id, accepted));
        if let Err(why) = answered {
            note(
                logger.as_deref(),
                &format!(
                    "pin shortcut: Android was not told the answer to request {}: {why}",
                    request.request_id
                ),
            );
        }
    }
}

/// The task dialog, from version 6 of the common controls. This program carries no manifest
/// that asks for that version, and the older one, which Windows loads without it, has no task
/// dialog, so the dialog is called through an activation context made from a manifest of its
/// own and looked up at run time: a program that named it at link time would not start.
mod dialog {
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows_sys::Win32::System::ApplicationInstallationAndServicing::{
        ActivateActCtx, CreateActCtxW, DeactivateActCtx, ReleaseActCtx, ACTCTXW,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows_sys::Win32::UI::Controls::{
        TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_CALLBACK_TIMER,
        TDF_USE_HICON_MAIN, TDM_CLICK_BUTTON, TDN_CREATED, TDN_TIMER,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, DestroyIcon, MessageBoxW, SendMessageW, SetForegroundWindow,
        IDCANCEL, IDOK, LR_DEFAULTCOLOR, MB_ICONQUESTION, MB_OKCANCEL, MB_SETFOREGROUND,
    };

    use super::Question;

    /// The ID of the [Add to desktop] button.
    const ADD: i32 = 100;

    /// The manifest that asks for version 6 of the common controls.
    const MANIFEST: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
        "<assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\">",
        "<dependency><dependentAssembly><assemblyIdentity type=\"win32\" ",
        "name=\"Microsoft.Windows.Common-Controls\" version=\"6.0.0.0\" ",
        "processorArchitecture=\"*\" publicKeyToken=\"6595b64144ccf1df\" language=\"*\"/>",
        "</dependentAssembly></dependency></assembly>\n"
    );

    type TaskDialogIndirect = unsafe extern "system" fn(
        config: *const TASKDIALOGCONFIG,
        button: *mut i32,
        radio: *mut i32,
        verified: *mut i32,
    ) -> i32;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Clicks Cancel once the time the dialog was given has passed, and brings it to the
    /// front when it opens.
    unsafe extern "system" fn callback(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        _lparam: LPARAM,
        within_ms: isize,
    ) -> i32 {
        match msg as i32 {
            TDN_CREATED => {
                // SAFETY: the dialog's own window.
                unsafe { SetForegroundWindow(hwnd) };
            }
            TDN_TIMER if wparam as isize >= within_ms => {
                // SAFETY: the dialog's own window, which takes the click.
                unsafe { SendMessageW(hwnd, TDM_CLICK_BUTTON as u32, IDCANCEL as usize, 0) };
            }
            _ => {}
        }
        0
    }

    /// The task dialog function of version 6 of the common controls, loaded under an
    /// activation context that asks for it. `None` when it cannot be had.
    fn task_dialog() -> Option<TaskDialogIndirect> {
        let dir = crate::paths::UserRoot::current()?.tmp();
        std::fs::create_dir_all(&dir).ok()?;
        let manifest = dir.join("common-controls-6.manifest");
        if std::fs::read_to_string(&manifest).ok().as_deref() != Some(MANIFEST) {
            std::fs::write(&manifest, MANIFEST).ok()?;
        }
        let source = wide(&manifest.to_string_lossy());
        let context = ACTCTXW {
            cbSize: std::mem::size_of::<ACTCTXW>() as u32,
            lpSource: source.as_ptr(),
            ..Default::default()
        };
        // SAFETY: a context made from a manifest file, activated around the load only, then
        // released; the library stays loaded for the life of the process.
        unsafe {
            let handle = CreateActCtxW(&context);
            if handle.is_null() || handle as isize == -1 {
                return None;
            }
            let mut cookie = 0usize;
            let active = ActivateActCtx(handle, &mut cookie) != 0;
            let library = wide("comctl32.dll");
            let module = LoadLibraryW(library.as_ptr());
            if active {
                DeactivateActCtx(0, cookie);
            }
            ReleaseActCtx(handle);
            if module.is_null() {
                return None;
            }
            let found = GetProcAddress(module, c"TaskDialogIndirect".as_ptr().cast())?;
            Some(std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                TaskDialogIndirect,
            >(found))
        }
    }

    pub fn ask(question: &Question, within: Duration) -> bool {
        let Some(task_dialog) = task_dialog() else {
            return message_box(question);
        };
        let title = wide(&question.title);
        let instruction = wide(&question.instruction);
        let content = wide(&question.content);
        let add = wide(&question.add);
        let cancel = wide(&question.cancel);
        let buttons = [
            TASKDIALOG_BUTTON {
                nButtonID: ADD,
                pszButtonText: add.as_ptr(),
            },
            TASKDIALOG_BUTTON {
                nButtonID: IDCANCEL,
                pszButtonText: cancel.as_ptr(),
            },
        ];
        // SAFETY: PNG bytes the call reads; the icon is destroyed below.
        let icon = if question.icon_png.is_empty() {
            std::ptr::null_mut()
        } else {
            unsafe {
                CreateIconFromResourceEx(
                    question.icon_png.as_ptr(),
                    question.icon_png.len() as u32,
                    1,
                    0x0003_0000,
                    48,
                    48,
                    LR_DEFAULTCOLOR,
                )
            }
        };
        let mut config = TASKDIALOGCONFIG {
            cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
            dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_CALLBACK_TIMER,
            pszWindowTitle: title.as_ptr(),
            pszMainInstruction: instruction.as_ptr(),
            pszContent: content.as_ptr(),
            cButtons: buttons.len() as u32,
            pButtons: buttons.as_ptr(),
            nDefaultButton: ADD,
            pfCallback: Some(callback),
            lpCallbackData: within.as_millis().min(i32::MAX as u128) as isize,
            ..Default::default()
        };
        if !icon.is_null() {
            config.dwFlags |= TDF_USE_HICON_MAIN;
            config.Anonymous1.hMainIcon = icon;
        }
        let mut pressed = 0;
        // SAFETY: a configuration whose strings and buttons outlive the call, which returns
        // when the dialog closes.
        let result = unsafe {
            task_dialog(
                &config,
                &mut pressed,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if !icon.is_null() {
            // SAFETY: the icon made above, no longer shown.
            unsafe { DestroyIcon(icon) };
        }
        if result < 0 {
            return message_box(question);
        }
        pressed == ADD
    }

    /// The question in a message box, for a Windows whose task dialog could not be loaded.
    fn message_box(question: &Question) -> bool {
        let title = wide(&question.title);
        let text = wide(&format!("{}\n\n{}", question.instruction, question.content));
        // SAFETY: NUL-terminated strings that outlive the call.
        let pressed = unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OKCANCEL | MB_ICONQUESTION | MB_SETFOREGROUND,
            )
        };
        pressed == IDOK
    }
}
