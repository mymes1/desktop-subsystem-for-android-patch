//! Files dropped on a task window from Explorer.
//!
//! Each task window is an OLE drop target. A drop of files hands their paths to the window
//! host's [`DropHandler`] on a thread of its own, so the window never waits on a transfer:
//! each file goes to the window's app (`ACTION_SEND` to its package), and an APK goes to the
//! install flow instead, as opening it in Explorer would. Folders are left out; only files
//! cross.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use dsa_host_core::files::{dropped, Dropped, FileAction, FileTransfer};
use dsa_host_core::host_api::HostApiClient;
use windows::core::{implement, Ref, Result as ComResult};
use windows::Win32::Foundation::{HWND, POINTL};
use windows::Win32::System::Com::{
    IDataObject, ReleaseStgMedium, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, RevokeDragDrop, CF_HDROP,
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

use super::accessibility::ApiConnect;
use super::apk_install;
use super::launch::run_program;

/// What happens to the files dropped on the window of a task: `(task_id, paths)`.
pub type DropHandler = Arc<dyn Fn(i32, Vec<PathBuf>) + Send + Sync>;

/// The build the window host names itself by when it sends dropped files.
const BUILD: &str = "dsa-app-window drop";

/// Makes this thread able to take drops. Called once on the window thread, before any
/// window is registered; an error means the windows take no drops.
pub fn prepare_thread() -> Result<(), String> {
    // SAFETY: called on the window thread, which then runs a message loop.
    unsafe { OleInitialize(None) }
        .map_err(|e| format!("the windows cannot take dropped files: {e}"))
}

/// Registers `hwnd`, the window of `task_id`, as a drop target that hands what is dropped to
/// `handler`.
pub fn register(hwnd: isize, task_id: i32, handler: DropHandler) -> Result<(), String> {
    let target: IDropTarget = DropTarget { task_id, handler }.into();
    // SAFETY: a window of this thread, and a target COM keeps alive while it is registered.
    unsafe { RegisterDragDrop(HWND(hwnd as *mut _), &target) }
        .map_err(|e| format!("task {task_id}'s window cannot take dropped files: {e}"))
}

/// Takes `hwnd` off the drop targets, as it closes.
pub fn revoke(hwnd: isize) {
    // SAFETY: a window of this thread; one never registered only fails.
    let _ = unsafe { RevokeDragDrop(HWND(hwnd as *mut _)) };
}

#[implement(IDropTarget)]
struct DropTarget {
    task_id: i32,
    handler: DropHandler,
}

/// The paths of the files in `data`, if it carries files (`CF_HDROP`).
fn dropped_paths(data: &IDataObject) -> Option<Vec<PathBuf>> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: a valid FORMATETC; the medium is released below.
    let mut medium = unsafe { data.GetData(&format) }.ok()?;
    // SAFETY: a CF_HDROP medium holds an HDROP in its global handle.
    let drop = HDROP(unsafe { medium.u.hGlobal.0 });
    // SAFETY: asking for the count with no buffer.
    let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        // SAFETY: asking for the length of one name, then the name into a buffer that holds it.
        let len = unsafe { DragQueryFileW(drop, index, None) } as usize;
        let mut name = vec![0u16; len + 1];
        let written = unsafe { DragQueryFileW(drop, index, Some(&mut name)) } as usize;
        paths.push(PathBuf::from(String::from_utf16_lossy(&name[..written])));
    }
    // SAFETY: the medium GetData gave, released once.
    unsafe { ReleaseStgMedium(&mut medium) };
    Some(paths)
}

/// The effect a drag over the window has: a copy when it carries files.
fn effect_of(data: Option<&IDataObject>) -> DROPEFFECT {
    match data.and_then(dropped_paths) {
        Some(paths) if !paths.is_empty() => DROPEFFECT_COPY,
        _ => DROPEFFECT_NONE,
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> ComResult<()> {
        if !effect.is_null() {
            // SAFETY: OLE passes a valid effect to write.
            unsafe { *effect = effect_of(data.as_ref()) };
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> ComResult<()> {
        if !effect.is_null() {
            // SAFETY: OLE passes a valid effect to write; it keeps the one DragEnter chose when
            // the target says copy, which every drag of files gets.
            unsafe { *effect = DROPEFFECT_COPY };
        }
        Ok(())
    }

    fn DragLeave(&self) -> ComResult<()> {
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> ComResult<()> {
        let paths = data.as_ref().and_then(dropped_paths).unwrap_or_default();
        if !effect.is_null() {
            // SAFETY: OLE passes a valid effect to write.
            unsafe {
                *effect = if paths.is_empty() {
                    DROPEFFECT_NONE
                } else {
                    DROPEFFECT_COPY
                }
            };
        }
        if !paths.is_empty() {
            (self.handler)(self.task_id, paths);
        }
        Ok(())
    }
}

/// The handler of a window host: each dropped file is shared with `package`, and an APK is
/// installed, on a thread of its own. `api` dials the service's local API; `note` logs.
pub fn handler(
    package: String,
    api: ApiConnect,
    note: Arc<dyn Fn(&str) + Send + Sync>,
) -> DropHandler {
    Arc::new(move |task_id, paths| {
        let (package, api, worker_note) = (package.clone(), Arc::clone(&api), Arc::clone(&note));
        let spawned = std::thread::Builder::new()
            .name("dsa-app-window-drop".into())
            .spawn(move || deliver(&package, &api, &*worker_note, task_id, &paths));
        if let Err(error) = spawned {
            note(&format!("dropped files cannot be sent: {error}"));
        }
    })
}

/// Sends each of `paths` where it goes, and logs what became of it.
fn deliver(package: &str, api: &ApiConnect, note: &dyn Fn(&str), task_id: i32, paths: &[PathBuf]) {
    for item in dropped(paths) {
        let outcome = match &item {
            Dropped::Install(path) => install(path),
            Dropped::Share(path) => share(package, api, path),
            Dropped::Skipped(path, why) => Err(format!("{}: {why}", path.display())),
        };
        match outcome {
            Ok(done) => note(&format!("task {task_id}: {done}")),
            Err(why) => {
                note(&format!(
                    "task {task_id}: a dropped file was not sent: {why}"
                ));
                super::launch::tell_user(
                    "Desktop Subsystem for Android",
                    &format!("A dropped file could not be sent to the app. {why}."),
                );
            }
        }
    }
}

/// Installs a dropped APK the way opening it in Explorer does: in the install window, which
/// shows what it is first, or else through this program's own install.
fn install(path: &std::path::Path) -> Result<String, String> {
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    let installed_program = crate::app_window::launch::window_manager_path(Some(program.as_path()))
        .and_then(|manager| manager.parent().map(|dir| dir.join("dsa-app-window.exe")))
        .unwrap_or(program);
    match apk_install::install_window(&installed_program) {
        Some(window) => run_program(&window, &apk_install::install_window_arguments(path)),
        None => run_program(
            &installed_program,
            &format!(
                "{} \"{}\"",
                apk_install::INSTALL_APK_ARGUMENT,
                path.display()
            ),
        ),
    }
    .map(|()| format!("installing {}", path.display()))
}

/// Shares a dropped file with the window's app.
fn share(package: &str, api: &ApiConnect, path: &std::path::Path) -> Result<String, String> {
    let mut file = FileTransfer::read(path, FileAction::Send)?;
    file.package = package.to_string();
    let mut connector = api();
    HostApiClient::connect(&mut *connector, BUILD, Duration::from_secs(5))
        .map_err(|e| format!("the DSA service cannot be reached: {e}"))?
        .open_file(&file)
        .map_err(|e| e.to_string())?;
    Ok(format!("shared {} with {package}", path.display()))
}
