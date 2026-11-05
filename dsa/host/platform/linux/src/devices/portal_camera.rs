//! The camera through the desktop's camera portal, for a lender that cannot open
//! `/dev/video*` itself: one in a Flatpak or another sandbox, or on a desktop that gives
//! cameras only through PipeWire.
//!
//! The portal (`org.freedesktop.portal.Camera`) asks the user once whether DSA may use the
//! camera, and then hands over a PipeWire connection that reaches only the cameras. The
//! pictures are read from it by GStreamer's PipeWire source in a child process, which
//! scales and converts them to the NV12 size the guest asked for and writes them whole to a
//! pipe. The camera runs only while the child does: from the guest's open to its stop.
//!
//! [`HostCameras`] lists the cameras Video4Linux2 can open directly, and the portal's camera
//! when there are none.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Instant;

use dsa_host_core::camera::{Camera, CameraStream, Cameras, Facing, Frame, Mode};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

use super::camera::{encode_jpeg, V4l2Cameras};
use super::dbus::portal_request;
use super::Log;

const CAMERA: &str = "org.freedesktop.portal.Camera";

/// The id the portal's camera is listed under.
pub const PORTAL_CAMERA_ID: &str = "portal:camera";

/// The sizes the portal's camera is offered at; the child scales the camera's own to them.
const SIZES: [(u32, u32); 4] = [(640, 480), (1280, 720), (1920, 1080), (320, 240)];

/// The cameras this lender can reach: Video4Linux2's, else the portal's.
pub struct HostCameras {
    direct: V4l2Cameras,
    log: Log,
}

impl HostCameras {
    pub fn new(log: Log) -> HostCameras {
        HostCameras {
            direct: V4l2Cameras::new(log.clone()),
            log,
        }
    }
}

impl Cameras for HostCameras {
    fn list(&self) -> Vec<Camera> {
        let direct = self.direct.list();
        if direct.iter().any(|camera| !camera.blocked) {
            return direct;
        }
        match portal_camera_present() {
            Ok(true) => vec![portal_camera()],
            Ok(false) => direct,
            Err(why) => {
                (self.log)(&format!("the camera portal cannot be asked: {why}"));
                direct
            }
        }
    }

    fn open(&self, id: &str, mode: Mode, fps: u32) -> Result<Box<dyn CameraStream>, String> {
        if id == PORTAL_CAMERA_ID {
            Ok(Box::new(PortalStream::open(mode, fps)?))
        } else {
            self.direct.open(id, mode, fps)
        }
    }

    fn encode_jpeg(&self, frame: &Frame, quality: u8) -> Result<Vec<u8>, String> {
        encode_jpeg(frame, quality)
    }
}

/// The portal's camera as the guest is shown it.
pub fn portal_camera() -> Camera {
    Camera {
        id: PORTAL_CAMERA_ID.into(),
        name: "Camera".into(),
        // The portal does not say where the camera faces.
        facing: Facing::External,
        sensor_orientation: 0,
        modes: SIZES
            .iter()
            .map(|&(width, height)| Mode {
                width,
                height,
                min_fps: 1,
                max_fps: 30,
            })
            .collect(),
        blocked: false,
    }
}

fn portal_camera_present() -> Result<bool, String> {
    let connection = Connection::session().map_err(|e| e.to_string())?;
    let portal = super::dbus::proxy(
        &connection,
        super::dbus::PORTAL,
        super::dbus::PORTAL_PATH.into(),
        CAMERA,
    )?;
    portal
        .get_property::<bool>("IsCameraPresent")
        .map_err(|e| e.to_string())
}

/// Asks the portal for the camera and returns the PipeWire connection it gives.
fn open_remote() -> Result<OwnedFd, String> {
    let connection = Connection::session().map_err(|e| e.to_string())?;
    let token = format!("dsa_camera_{}", std::process::id());
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    let (code, _): (u32, HashMap<String, OwnedValue>) =
        portal_request(&connection, CAMERA, "AccessCamera", &token, |camera| {
            camera.call("AccessCamera", &(options,))
        })?;
    if code != 0 {
        return Err("the user did not let DSA use the camera".into());
    }
    let portal = super::dbus::proxy(
        &connection,
        super::dbus::PORTAL,
        super::dbus::PORTAL_PATH.into(),
        CAMERA,
    )?;
    let empty: HashMap<&str, Value<'_>> = HashMap::new();
    let fd: zbus::zvariant::OwnedFd = portal
        .call("OpenPipeWireRemote", &(empty,))
        .map_err(|e| format!("the camera portal gave no PipeWire connection: {e}"))?;
    Ok(fd.into())
}

/// The GStreamer pipeline that reads the portal's camera from the PipeWire connection on
/// descriptor 3 and writes NV12 pictures of `mode`'s size to standard output.
pub fn pipeline(mode: Mode, fps: u32) -> Vec<String> {
    let fps = if fps == 0 { mode.max_fps } else { fps }.clamp(1, 60);
    [
        "-q",
        "pipewiresrc",
        "fd=3",
        "do-timestamp=true",
        "!",
        "videoconvert",
        "!",
        "videoscale",
        "!",
        "videorate",
        "!",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([
        format!(
            "video/x-raw,format=NV12,width={},height={},framerate={fps}/1",
            mode.width, mode.height
        ),
        "!".into(),
        "fdsink".into(),
        "fd=1".into(),
        "sync=false".into(),
    ])
    .collect()
}

/// The portal's camera, running.
struct PortalStream {
    child: Child,
    output: ChildStdout,
    mode: Mode,
    started: Instant,
}

impl PortalStream {
    fn open(mode: Mode, fps: u32) -> Result<PortalStream, String> {
        if mode.width % 2 != 0 || mode.height % 2 != 0 {
            return Err("the camera's size must be even".into());
        }
        let remote = open_remote()?;
        let raw = remote.as_raw_fd();
        let mut command = Command::new("gst-launch-1.0");
        command
            .args(pipeline(mode, fps))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // SAFETY: only dup2 runs between fork and exec; it is async-signal-safe. The copy on
        // descriptor 3 has no close-on-exec flag, so the child keeps it.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(raw, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("GStreamer (gst-launch-1.0) cannot run: {e}"))?;
        drop(remote);
        let output = child
            .stdout
            .take()
            .ok_or("the camera's reader gave no output")?;
        Ok(PortalStream {
            child,
            output,
            mode,
            started: Instant::now(),
        })
    }
}

impl CameraStream for PortalStream {
    fn next(&mut self) -> Result<Frame, String> {
        let (width, height) = (self.mode.width, self.mode.height);
        let len = Frame::nv12_len(width, height, width).ok_or("the camera's size is not valid")?;
        let mut nv12 = vec![0u8; len];
        self.output
            .read_exact(&mut nv12)
            .map_err(|e| format!("the camera stopped: {e}"))?;
        Ok(Frame {
            width,
            height,
            stride: width,
            nv12,
            timestamp_ns: self.started.elapsed().as_nanos() as u64,
        })
    }
}

impl Drop for PortalStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_the_portals_camera_is_offered_in_sizes_the_guest_takes() {
        let camera = portal_camera();
        let offered = dsa_host_core::camera::offered_modes(&camera.modes);
        assert_eq!(offered.len(), SIZES.len());
        assert!(!camera.blocked);
        assert_eq!(camera.facing, Facing::External);
    }

    #[test]
    fn fr26_the_portals_pictures_come_as_nv12_of_the_asked_size() {
        let mode = Mode {
            width: 1280,
            height: 720,
            min_fps: 1,
            max_fps: 30,
        };
        let line = pipeline(mode, 0).join(" ");
        assert!(line.starts_with("-q pipewiresrc fd=3 "), "{line}");
        assert!(
            line.contains("video/x-raw,format=NV12,width=1280,height=720,framerate=30/1"),
            "{line}"
        );
        assert!(line.ends_with("fdsink fd=1 sync=false"), "{line}");
        assert!(pipeline(mode, 15).join(" ").contains("framerate=15/1"));
    }
}
