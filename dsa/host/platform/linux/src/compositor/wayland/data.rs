//! The clipboard and drag and drop (`wl_data_device`).
//!
//! The desktop's clipboard reaches the guest whenever it changes while a task window has the
//! keyboard, which is when Wayland lets a program read it. What the guest copies becomes the
//! desktop's clipboard, offered under every name for text. Files dropped on a task window
//! are handed to the window's app.

use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::{Arc, Mutex};

use wayland_client::protocol::{
    wl_data_device::{self, WlDataDevice},
    wl_data_device_manager::DndAction,
    wl_data_offer::{self, WlDataOffer},
    wl_data_source::{self, WlDataSource},
};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};

use super::{task_of, Event, State};
use crate::shell::clipboard::{host_text, text_type, TEXT_TYPES};
use crate::shell::file_drop::{paths, URI_LIST};

/// The types an offer has, as it announces them.
pub type OfferTypes = Mutex<Vec<String>>;

#[derive(Default)]
pub struct DataState {
    /// The offer being dragged over a task window, with the task.
    drag: Option<(WlDataOffer, i32)>,
    /// The source of what the guest copied last, while it is the desktop's clipboard.
    source: Option<WlDataSource>,
    /// Text the guest copied while no task window had the keyboard, to set when one has.
    waiting: Option<String>,
    /// The desktop's clipboard offer, read when a task window has the keyboard.
    selection: Option<WlDataOffer>,
}

/// A pipe for an offer to write into: (read end, write end).
fn pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` takes the two new descriptors, owned from here on.
    unsafe {
        use std::os::fd::FromRawFd;
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return None;
        }
        Some((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])))
    }
}

/// Reads all an offer writes, on a thread of its own so the desktop never waits on us (or we
/// on ourselves, when the offer is our own).
fn read_offer(
    offer: &WlDataOffer,
    mime: &str,
    connection: &Connection,
    done: impl FnOnce(Vec<u8>) + Send + 'static,
) {
    let Some((read, write)) = pipe() else {
        return;
    };
    offer.receive(mime.to_string(), write.as_fd());
    drop(write);
    let _ = connection.flush();
    std::thread::spawn(move || {
        let mut file = std::fs::File::from(read);
        let mut bytes = Vec::new();
        let limit = dsa_host_core::clipboard::MAX_CLIPBOARD_BYTES as u64 * 4 + 1;
        if (&mut file).take(limit).read_to_end(&mut bytes).is_ok() {
            done(bytes);
        }
    });
}

impl State {
    /// A task window got the keyboard: the desktop's clipboard may be read now, and what the
    /// guest copied meanwhile becomes the desktop's.
    pub(crate) fn data_focus_changed(&mut self) {
        if let Some(text) = self.data_state.waiting.take() {
            self.set_clipboard(text);
        }
        self.read_selection();
    }

    fn read_selection(&mut self) {
        if self.seat_state.keyboard_focus.is_none() {
            return;
        }
        let Some(offer) = self.data_state.selection.clone() else {
            return;
        };
        let types = offer
            .data::<OfferTypes>()
            .and_then(|types| types.lock().ok().map(|types| types.clone()))
            .unwrap_or_default();
        let Some(mime) = text_type(&types).map(str::to_string) else {
            return;
        };
        let events = self.events.clone();
        read_offer(&offer, &mime, &self.connection, move |bytes| {
            if let Some(text) = host_text(bytes) {
                events.send(Event::HostClipboard(text));
            }
        });
    }

    /// The guest copied `text`: it becomes the desktop's clipboard.
    pub fn set_clipboard(&mut self, text: String) {
        let (Some(manager), Some(device)) = (&self.data_manager, &self.data_device) else {
            return;
        };
        if self.seat_state.keyboard_focus.is_none() {
            self.data_state.waiting = Some(text);
            return;
        }
        let source = manager.create_data_source(&self.qh, Arc::new(text));
        for mime in TEXT_TYPES {
            source.offer(mime.to_string());
        }
        device.set_selection(Some(&source), self.serial);
        if let Some(old) = self.data_state.source.replace(source) {
            old.destroy();
        }
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Selection { id } => {
                if let Some(old) = std::mem::replace(&mut state.data_state.selection, id) {
                    old.destroy();
                }
                // What the guest copied comes back as the desktop's selection too; the
                // session drops text that only echoes the guest's own.
                state.read_selection();
            }
            wl_data_device::Event::Enter {
                serial,
                surface,
                id,
                ..
            } => {
                let (Some(offer), Some(task_id)) = (id, task_of(&surface)) else {
                    return;
                };
                let has_files = offer
                    .data::<OfferTypes>()
                    .and_then(|types| types.lock().ok().map(|t| t.iter().any(|t| t == URI_LIST)))
                    .unwrap_or(false);
                if has_files {
                    offer.accept(serial, Some(URI_LIST.to_string()));
                    if offer.version() >= 3 {
                        offer.set_actions(DndAction::Copy, DndAction::Copy);
                    }
                    state.data_state.drag = Some((offer, task_id));
                } else {
                    offer.accept(serial, None);
                }
            }
            wl_data_device::Event::Leave => {
                if let Some((offer, _)) = state.data_state.drag.take() {
                    offer.destroy();
                }
            }
            wl_data_device::Event::Drop => {
                let Some((offer, task_id)) = state.data_state.drag.take() else {
                    return;
                };
                let package = state
                    .windows
                    .get(&task_id)
                    .map(|window| window.model.package.clone())
                    .unwrap_or_default();
                let events = state.events.clone();
                let finished = offer.clone();
                read_offer(&offer, URI_LIST, &state.connection, move |bytes| {
                    let list = String::from_utf8_lossy(&bytes);
                    let paths = paths(&list);
                    if !paths.is_empty() {
                        events.send(Event::Dropped {
                            task_id,
                            package,
                            paths,
                        });
                    }
                });
                if finished.version() >= 3 {
                    finished.finish();
                }
                finished.destroy();
            }
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Mutex::new(Vec::new())),
    ]);
}

impl Dispatch<WlDataOffer, OfferTypes> for State {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        types: &OfferTypes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            if let Ok(mut types) = types.lock() {
                types.push(mime_type);
            }
        }
    }
}

impl Dispatch<WlDataSource, Arc<String>> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        text: &Arc<String>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                let text = text.clone();
                std::thread::spawn(move || {
                    let mut file = std::fs::File::from(fd);
                    let _ = file.write_all(text.as_bytes());
                });
            }
            wl_data_source::Event::Cancelled => {
                if state.data_state.source.as_ref() == Some(source) {
                    state.data_state.source = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}
