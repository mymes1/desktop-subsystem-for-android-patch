//! The guest's graphics streams.
//!
//! Only the process that owns the VM can accept the guest's vsock connections, so the agent
//! (`dsa-hostd`) relays each connection to the graphics ports to a Unix socket of the window
//! manager's, `<user folder>/vsock/<port>.sock`. The window manager serves each one on a
//! thread of its own through the renderer: the command stream on port 5566, the process pipe
//! on 5567. A stream that breaks ends its own thread and nothing else; a VM that restarts
//! simply connects again.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use dsa_host_core::compositor::GraphicsStreamKind;

use super::log;
use crate::compositor::renderer::Renderer;

/// Listens on `socket` for the guest's streams of `kind`, and serves each through
/// `renderer`, until `stop` is set. Without a renderer the streams are refused, and the
/// windows open without frames.
pub fn serve(
    socket: PathBuf,
    kind: GraphicsStreamKind,
    renderer: Option<Arc<Renderer>>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let listener = bind(&socket)?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("listening on {}: {e}", socket.display()))?;
    thread::Builder::new()
        .name(format!("dsa-wm-graphics-{}", kind.port()))
        .spawn(move || {
            let mut refused_logged = false;
            while !stop.load(Ordering::Relaxed) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    Err(error) => {
                        log(&format!("a graphics stream could not be taken: {error}"));
                        thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                };
                let Some(renderer) = renderer.clone() else {
                    if !refused_logged {
                        log("graphics: no renderer is loaded; Android's frames are refused");
                        refused_logged = true;
                    }
                    continue;
                };
                let _ = thread::Builder::new()
                    .name(format!("dsa-wm-stream-{}", kind.port()))
                    .spawn(move || serve_one(stream, kind, &renderer));
            }
            let _ = std::fs::remove_file(&socket);
        })
        .map_err(|e| format!("the graphics streams cannot be served: {e}"))?;
    Ok(())
}

fn bind(socket: &Path) -> Result<UnixListener, String> {
    if let Some(folder) = socket.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("creating {}: {e}", folder.display()))?;
    }
    // A socket left by an earlier run is replaced; the agent dials whoever listens now.
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)
        .map_err(|e| format!("listening on {}: {e}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("protecting {}: {e}", socket.display()))?;
    Ok(listener)
}

fn serve_one(mut stream: UnixStream, kind: GraphicsStreamKind, renderer: &Renderer) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    // A panic in a stream's thread must not reach the renderer's other streams.
    let served = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match kind {
        GraphicsStreamKind::Command => renderer.serve_stream(&mut stream),
        GraphicsStreamKind::ProcessPipe => renderer.serve_process(&mut stream),
    }));
    match served {
        Ok(Ok(())) => {}
        Ok(Err(why)) => log(&format!("graphics: {why}")),
        Err(_) => log("graphics: a stream's thread failed; the other streams go on"),
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}
