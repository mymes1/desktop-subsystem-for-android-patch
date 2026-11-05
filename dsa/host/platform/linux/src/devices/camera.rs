//! The computer's cameras, through Video4Linux2.
//!
//! Each `/dev/video*` node that captures video is one camera. Its sizes and rates come from
//! the driver, in whichever of the formats this converts it offers (NV12, YUYV, I420, Motion
//! JPEG); each picture goes to the guest as NV12. A camera opens only while an app in the
//! guest uses it, so the camera's light is on only then.
//!
//! A node the user may not open (not in the `video` group, or a desktop that has taken the
//! right away) is listed as blocked, which the guest does not see. Where the camera faces
//! comes from the driver's orientation control when it has one; a camera built into the
//! computer that does not say faces the user, and any other is external.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use dsa_host_core::camera::{
    sensor_orientation, Camera, CameraStream, Cameras, Facing, Frame, Mode,
};

use super::v4l2::{self, Capture, Device};
use super::Log;

/// The formats this converts, the most direct first.
const FORMATS: [u32; 5] = [
    v4l2::PIX_NV12,
    v4l2::PIX_YUYV,
    v4l2::PIX_YU12,
    v4l2::PIX_MJPEG,
    v4l2::PIX_JPEG,
];

/// How long a picture is waited for before the camera is taken as stalled.
const PICTURE_WAIT: Duration = Duration::from_secs(5);

/// The cameras of this computer.
pub struct V4l2Cameras {
    log: Log,
    /// For each camera listed, the format to capture each size in.
    formats: Mutex<HashMap<String, Vec<(u32, u32, u32)>>>,
}

impl V4l2Cameras {
    pub fn new(log: Log) -> V4l2Cameras {
        V4l2Cameras {
            log,
            formats: Mutex::new(HashMap::new()),
        }
    }

    fn nodes() -> Vec<PathBuf> {
        let mut nodes: Vec<PathBuf> = fs::read_dir("/dev")
            .map(|dir| {
                dir.flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("video"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        nodes.sort_by_key(|path| node_number(path));
        nodes
    }

    /// The camera at `path`, or `None` when the node does not capture video.
    fn probe(&self, path: &Path) -> Option<(Camera, Vec<(u32, u32, u32)>)> {
        let id = path.to_string_lossy().into_owned();
        let device = match Device::open(path) {
            Ok(device) => device,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Some((
                    Camera {
                        name: id.clone(),
                        id,
                        facing: Facing::External,
                        sensor_orientation: 0,
                        modes: Vec::new(),
                        blocked: true,
                    },
                    Vec::new(),
                ));
            }
            Err(_) => return None,
        };
        let cap = device.capability().ok()?;
        let caps = if cap.capabilities & v4l2::CAP_DEVICE_CAPS != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };
        if caps & v4l2::CAP_VIDEO_CAPTURE == 0 || caps & v4l2::CAP_STREAMING == 0 {
            return None;
        }
        let name = v4l2::text(&cap.card);
        let offered = device.formats();
        let mut by_size: Vec<(u32, u32, u32)> = Vec::new();
        let mut modes = Vec::new();
        for format in FORMATS.iter().copied().filter(|f| offered.contains(f)) {
            for (width, height) in device.sizes(format) {
                let Some((min_fps, max_fps)) = device.rates(format, width, height) else {
                    continue;
                };
                // A size comes in the first (most direct) format that has it, unless a later
                // one delivers it faster, as Motion JPEG often does at large sizes.
                match by_size.iter_mut().position(|(w, h, _)| *w == width && *h == height) {
                    Some(at) => {
                        let known = modes
                            .iter_mut()
                            .find(|m: &&mut Mode| m.width == width && m.height == height);
                        if let Some(known) = known {
                            if max_fps > known.max_fps {
                                known.max_fps = max_fps;
                                known.min_fps = known.min_fps.min(min_fps);
                                by_size[at].2 = format;
                            }
                        }
                    }
                    None => {
                        by_size.push((width, height, format));
                        modes.push(Mode {
                            width,
                            height,
                            min_fps,
                            max_fps,
                        });
                    }
                }
            }
        }
        if modes.is_empty() {
            (self.log)(&format!(
                "camera {name} ({id}) is not lent to Android: it delivers no format DSA converts"
            ));
            return None;
        }
        let facing = match device.control(v4l2::CID_CAMERA_ORIENTATION) {
            Some(0) => Facing::Front,
            Some(1) => Facing::Back,
            Some(_) => Facing::External,
            None => facing_from_name(&name, &v4l2::text(&cap.bus_info)),
        };
        let rotation = device
            .control(v4l2::CID_CAMERA_SENSOR_ROTATION)
            .unwrap_or(0);
        Some((
            Camera {
                id,
                name,
                facing,
                sensor_orientation: sensor_orientation(i64::from(rotation)),
                modes,
                blocked: false,
            },
            by_size,
        ))
    }
}

fn node_number(path: &Path) -> u32 {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("video"))
        .and_then(|n| n.parse().ok())
        .unwrap_or(u32::MAX)
}

/// Where a camera faces when its driver does not say: one built into the computer faces the
/// user, as a laptop's does; any other is external.
pub fn facing_from_name(card: &str, bus_info: &str) -> Facing {
    let card = card.to_ascii_lowercase();
    let built_in = ["integrated", "built-in", "internal", "facetime", "user facing"]
        .iter()
        .any(|word| card.contains(word));
    // Cameras on a laptop's own image processor (IPU, MIPI) are not on a USB port.
    let on_board = !bus_info.is_empty() && !bus_info.starts_with("usb");
    if built_in || on_board {
        Facing::Front
    } else {
        Facing::External
    }
}

impl Cameras for V4l2Cameras {
    fn list(&self) -> Vec<Camera> {
        let mut cameras = Vec::new();
        let mut formats = HashMap::new();
        for node in Self::nodes() {
            if let Some((camera, by_size)) = self.probe(&node) {
                formats.insert(camera.id.clone(), by_size);
                cameras.push(camera);
            }
        }
        *self.formats.lock().unwrap_or_else(|e| e.into_inner()) = formats;
        cameras
    }

    fn open(&self, id: &str, mode: Mode, fps: u32) -> Result<Box<dyn CameraStream>, String> {
        let path = Path::new(id);
        if !id.starts_with("/dev/video") || node_number(path) == u32::MAX {
            return Err(format!("{id} is not a camera of this computer"));
        }
        let format = {
            let known = self.formats.lock().unwrap_or_else(|e| e.into_inner());
            known.get(id).and_then(|sizes| {
                sizes
                    .iter()
                    .find(|(w, h, _)| *w == mode.width && *h == mode.height)
                    .map(|(_, _, f)| *f)
            })
        };
        let format = match format {
            Some(format) => format,
            None => {
                let (_, sizes) = self
                    .probe(path)
                    .ok_or_else(|| format!("camera {id} is gone"))?;
                sizes
                    .iter()
                    .find(|(w, h, _)| *w == mode.width && *h == mode.height)
                    .map(|(_, _, f)| *f)
                    .ok_or_else(|| {
                        format!("camera {id} has no {}x{} mode", mode.width, mode.height)
                    })?
            }
        };
        let device = Device::open(path).map_err(|e| format!("opening camera {id}: {e}"))?;
        let capture = Capture::start(device, format, mode.width, mode.height, fps)
            .map_err(|e| format!("starting camera {id}: {e}"))?;
        Ok(Box::new(V4l2Stream { capture }))
    }

    fn encode_jpeg(&self, frame: &Frame, quality: u8) -> Result<Vec<u8>, String> {
        encode_jpeg(frame, quality)
    }
}

/// A camera capturing for the guest.
struct V4l2Stream {
    capture: Capture,
}

impl CameraStream for V4l2Stream {
    fn next(&mut self) -> Result<Frame, String> {
        let format = self.capture.format;
        self.capture
            .next(PICTURE_WAIT, |bytes, timestamp_ns| {
                to_nv12(
                    format.pixelformat,
                    format.width,
                    format.height,
                    format.bytesperline,
                    bytes,
                )
                .map(|mut frame| {
                    frame.timestamp_ns = timestamp_ns;
                    frame
                })
            })
            .map_err(|e| format!("the camera stopped: {e}"))?
    }
}

/// An empty NV12 picture of `width` by `height`.
fn blank(width: u32, height: u32) -> Result<Frame, String> {
    let stride = width + (width & 1);
    let len = Frame::nv12_len(width, height, stride)
        .ok_or_else(|| format!("a picture of {width}x{height} cannot be made"))?;
    Ok(Frame {
        width,
        height,
        stride,
        nv12: vec![0; len],
        timestamp_ns: 0,
    })
}

/// Turns a picture the camera delivered in `format` into NV12.
pub fn to_nv12(
    format: u32,
    width: u32,
    height: u32,
    bytes_per_line: u32,
    bytes: &[u8],
) -> Result<Frame, String> {
    match format {
        v4l2::PIX_NV12 => nv12_from_nv12(width, height, bytes_per_line.max(width), bytes),
        v4l2::PIX_YUYV => nv12_from_yuyv(width, height, bytes_per_line.max(width * 2), bytes),
        v4l2::PIX_YU12 => nv12_from_i420(width, height, bytes_per_line.max(width), bytes),
        v4l2::PIX_MJPEG | v4l2::PIX_JPEG => nv12_from_jpeg(bytes),
        other => Err(format!("the camera's format {other:#x} is not converted")),
    }
}

fn short(what: &str) -> String {
    format!("the camera sent a {what} picture shorter than its size")
}

fn nv12_from_nv12(width: u32, height: u32, line: u32, bytes: &[u8]) -> Result<Frame, String> {
    let mut frame = blank(width, height)?;
    let (w, h, line, stride) = (
        width as usize,
        height as usize,
        line as usize,
        frame.stride as usize,
    );
    let chroma_rows = h.div_ceil(2);
    if bytes.len() < line * (h + chroma_rows) - (line - w) {
        return Err(short("NV12"));
    }
    for row in 0..h + chroma_rows {
        let from = row * line;
        frame.nv12[row * stride..row * stride + w].copy_from_slice(&bytes[from..from + w]);
    }
    Ok(frame)
}

fn nv12_from_yuyv(width: u32, height: u32, line: u32, bytes: &[u8]) -> Result<Frame, String> {
    let mut frame = blank(width, height)?;
    let (w, h, line, stride) = (
        width as usize,
        height as usize,
        line as usize,
        frame.stride as usize,
    );
    if bytes.len() < line * (h - 1) + w * 2 {
        return Err(short("YUYV"));
    }
    let chroma = stride * h;
    for y in 0..h {
        let row = &bytes[y * line..y * line + w * 2];
        for x in 0..w {
            frame.nv12[y * stride + x] = row[x * 2];
        }
        if y % 2 == 0 {
            // Each chroma sample is the mean of the two rows it stands for.
            let below = if y + 1 < h {
                &bytes[(y + 1) * line..(y + 1) * line + w * 2]
            } else {
                row
            };
            for pair in 0..w / 2 {
                let at = pair * 4;
                let u = (u16::from(row[at + 1]) + u16::from(below[at + 1])) / 2;
                let v = (u16::from(row[at + 3]) + u16::from(below[at + 3])) / 2;
                let to = chroma + (y / 2) * stride + pair * 2;
                frame.nv12[to] = u as u8;
                frame.nv12[to + 1] = v as u8;
            }
        }
    }
    Ok(frame)
}

fn nv12_from_i420(width: u32, height: u32, line: u32, bytes: &[u8]) -> Result<Frame, String> {
    let mut frame = blank(width, height)?;
    let (w, h, line, stride) = (
        width as usize,
        height as usize,
        line as usize,
        frame.stride as usize,
    );
    let (cw, ch, cline) = (w.div_ceil(2), h.div_ceil(2), line.div_ceil(2));
    let u_at = line * h;
    let v_at = u_at + cline * ch;
    if bytes.len() < v_at + cline * ch {
        return Err(short("I420"));
    }
    for y in 0..h {
        frame.nv12[y * stride..y * stride + w].copy_from_slice(&bytes[y * line..y * line + w]);
    }
    let chroma = stride * h;
    for y in 0..ch {
        for x in 0..cw {
            let to = chroma + y * stride + x * 2;
            frame.nv12[to] = bytes[u_at + y * cline + x];
            frame.nv12[to + 1] = bytes[v_at + y * cline + x];
        }
    }
    Ok(frame)
}

/// The BT.601 limited-range Y, U and V of an RGB pixel.
fn yuv(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (
        y.clamp(0, 255) as u8,
        u.clamp(0, 255) as u8,
        v.clamp(0, 255) as u8,
    )
}

/// NV12 from tightly packed RGB pixels.
pub fn nv12_from_rgb(width: u32, height: u32, rgb: &[u8]) -> Result<Frame, String> {
    let mut frame = blank(width, height)?;
    let (w, h, stride) = (width as usize, height as usize, frame.stride as usize);
    if rgb.len() < w * h * 3 {
        return Err(short("RGB"));
    }
    let chroma = stride * h;
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) * 3;
            let (luma, u, v) = yuv(rgb[at], rgb[at + 1], rgb[at + 2]);
            frame.nv12[y * stride + x] = luma;
            if y % 2 == 0 && x % 2 == 0 {
                let to = chroma + (y / 2) * stride + x;
                frame.nv12[to] = u;
                frame.nv12[to + 1] = v;
            }
        }
    }
    Ok(frame)
}

fn nv12_from_jpeg(bytes: &[u8]) -> Result<Frame, String> {
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(bytes, options);
    let pixels = decoder
        .decode()
        .map_err(|e| format!("the camera sent a picture that does not decode: {e:?}"))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or("the camera sent a picture of no size")?;
    let (width, height) = (width as u32 & !1, height as u32 & !1);
    let full_width = decoder.dimensions().map_or(0, |(w, _)| w);
    if full_width as u32 == width {
        return nv12_from_rgb(width, height, &pixels);
    }
    // An odd width loses its last column, as NV12 needs an even one.
    let mut even = Vec::with_capacity(width as usize * height as usize * 3);
    for row in pixels.chunks_exact(full_width * 3).take(height as usize) {
        even.extend_from_slice(&row[..width as usize * 3]);
    }
    nv12_from_rgb(width, height, &even)
}

/// Encodes an NV12 picture as JPEG at `quality` (1 to 100).
pub fn encode_jpeg(frame: &Frame, quality: u8) -> Result<Vec<u8>, String> {
    let (w, h, stride) = (
        frame.width as usize,
        frame.height as usize,
        frame.stride as usize,
    );
    if Frame::nv12_len(frame.width, frame.height, frame.stride) != Some(frame.nv12.len()) {
        return Err("the picture is not whole".into());
    }
    let width = u16::try_from(frame.width).map_err(|_| "the picture is too wide for JPEG")?;
    let height = u16::try_from(frame.height).map_err(|_| "the picture is too tall for JPEG")?;
    let chroma = stride * h;
    let mut ycbcr = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let c = chroma + (y / 2) * stride + (x / 2) * 2;
            ycbcr.extend_from_slice(&[
                frame.nv12[y * stride + x],
                frame.nv12[c],
                frame.nv12[c + 1],
            ]);
        }
    }
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    encoder
        .encode(&ycbcr, width, height, jpeg_encoder::ColorType::Ycbcr)
        .map_err(|e| format!("the picture cannot be encoded: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_yuyv_pictures_become_nv12() {
        // 2x2: luma 10, 20 / 30, 40; chroma U 100 and 120, V 200 and 220 over the two rows.
        let yuyv = [10, 100, 20, 200, 30, 120, 40, 220];
        let frame = to_nv12(v4l2::PIX_YUYV, 2, 2, 4, &yuyv).unwrap();
        assert_eq!(frame.nv12, vec![10, 20, 30, 40, 110, 210]);
    }

    #[test]
    fn fr26_i420_pictures_become_nv12() {
        let i420 = [1, 2, 3, 4, 50, 60];
        let frame = to_nv12(v4l2::PIX_YU12, 2, 2, 2, &i420).unwrap();
        assert_eq!(frame.nv12, vec![1, 2, 3, 4, 50, 60]);
    }

    #[test]
    fn fr26_nv12_pictures_drop_the_drivers_padding() {
        let padded = [1, 2, 0, 0, 3, 4, 0, 0, 7, 8];
        let frame = to_nv12(v4l2::PIX_NV12, 2, 2, 4, &padded).unwrap();
        assert_eq!(frame.nv12, vec![1, 2, 3, 4, 7, 8]);
    }

    #[test]
    fn fr26_a_short_picture_is_an_error_not_a_crash() {
        assert!(to_nv12(v4l2::PIX_YUYV, 640, 480, 1280, &[0; 16]).is_err());
        assert!(to_nv12(v4l2::PIX_MJPEG, 640, 480, 0, &[0xff, 0xd8]).is_err());
    }

    #[test]
    fn fr26_a_still_picture_is_a_jpeg_that_decodes_to_its_size() {
        let frame = nv12_from_rgb(16, 8, &[128; 16 * 8 * 3]).unwrap();
        let jpeg = encode_jpeg(&frame, 90).unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        let back = nv12_from_jpeg(&jpeg).unwrap();
        assert_eq!((back.width, back.height), (16, 8));
    }

    #[test]
    fn fr26_built_in_cameras_face_the_user_and_others_are_external() {
        assert_eq!(
            facing_from_name("Integrated Camera: Integrated C", "usb-0000:00:14.0-8"),
            Facing::Front
        );
        assert_eq!(
            facing_from_name("Logitech BRIO", "usb-0000:00:14.0-2"),
            Facing::External
        );
        assert_eq!(facing_from_name("ov5693", "platform:ipu6"), Facing::Front);
    }
}
