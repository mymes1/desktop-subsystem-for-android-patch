//! The Mac's cameras through AVFoundation, as NV12 pictures for the guest's camera HAL.
//!
//! A camera delivers bi-planar 4:2:0 video-range buffers (`420v`), which are NV12: the luma
//! plane and the interleaved chroma plane are copied into one [`Frame`]. A picture the guest
//! asks for as a still is encoded to JPEG through `NSBitmapImageRep`.

use dsa_host_core::camera::{Facing, Frame};

/// The facing of a camera at AVFoundation's `position` (0 unspecified, 1 back, 2 front) of
/// device type `device_type`. A Mac's built-in camera has no position and faces the user.
pub fn facing(position: isize, device_type: &str) -> Facing {
    match position {
        1 => Facing::Back,
        2 => Facing::Front,
        _ if device_type.contains("External") || device_type.contains("Continuity") => {
            Facing::External
        }
        _ => Facing::Front,
    }
}

/// The stride a frame of `width` gets: the width, rounded up to an even number of bytes so
/// the chroma pairs line up.
pub fn stride(width: u32) -> u32 {
    width + (width & 1)
}

/// One plane of a locked pixel buffer.
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    pub bytes: &'a [u8],
    pub bytes_per_row: usize,
}

/// A frame from the two planes of a bi-planar 4:2:0 buffer of `width` by `height`. `None`
/// when a plane is shorter than its size says.
pub fn pack_nv12(
    width: u32,
    height: u32,
    luma: Plane<'_>,
    chroma: Plane<'_>,
    timestamp_ns: u64,
) -> Option<Frame> {
    let stride = stride(width);
    let len = Frame::nv12_len(width, height, stride)?;
    let mut nv12 = vec![0u8; len];
    let (w, h, s) = (width as usize, height as usize, stride as usize);
    let chroma_rows = h.div_ceil(2);
    if luma.bytes_per_row < w || chroma.bytes_per_row < stride as usize {
        return None;
    }
    if luma.bytes.len() < luma.bytes_per_row * (h - 1) + w
        || chroma.bytes.len() < chroma.bytes_per_row * (chroma_rows - 1) + s
    {
        return None;
    }
    for row in 0..h {
        let from = &luma.bytes[row * luma.bytes_per_row..][..w];
        nv12[row * s..][..w].copy_from_slice(from);
    }
    let base = s * h;
    for row in 0..chroma_rows {
        let from = &chroma.bytes[row * chroma.bytes_per_row..][..s];
        nv12[base + row * s..][..s].copy_from_slice(from);
    }
    Some(Frame {
        width,
        height,
        stride,
        nv12,
        timestamp_ns,
    })
}

/// `frame` as 8-bit RGB, three bytes a pixel, from BT.601 video range.
pub fn nv12_to_rgb(frame: &Frame) -> Vec<u8> {
    let (w, h, s) = (
        frame.width as usize,
        frame.height as usize,
        frame.stride as usize,
    );
    let mut rgb = Vec::with_capacity(w * h * 3);
    let chroma = &frame.nv12[s * h..];
    for y in 0..h {
        for x in 0..w {
            let luma = f32::from(frame.nv12[y * s + x]) - 16.0;
            let at = (y / 2) * s + (x / 2) * 2;
            let u = f32::from(chroma[at]) - 128.0;
            let v = f32::from(chroma[at + 1]) - 128.0;
            let c = 1.164 * luma;
            let clamp = |value: f32| value.round().clamp(0.0, 255.0) as u8;
            rgb.push(clamp(c + 1.596 * v));
            rgb.push(clamp(c - 0.392 * u - 0.813 * v));
            rgb.push(clamp(c + 2.017 * u));
        }
    }
    rgb
}

/// Whether a frame taken `since_last_ns` after the last one passed on is due at `fps`. A
/// camera runs at the rates its mode offers, and frames above what the guest asked for are
/// dropped here; a little early still counts, so 30 a second from a camera at 30 all pass.
pub fn frame_due(since_last_ns: u64, fps: u32) -> bool {
    if fps == 0 {
        return true;
    }
    let period = 1_000_000_000 / u64::from(fps);
    since_last_ns >= period - period / 5
}

#[cfg(target_os = "macos")]
pub use avfoundation::MacCameras;

#[cfg(target_os = "macos")]
mod avfoundation {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use dsa_host_core::camera::{Camera, CameraStream, Cameras, Frame, Mode};
    use objc2::encode::{Encoding, RefEncode};
    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
    use objc2::sel;

    use super::{facing, frame_due, nv12_to_rgb, pack_nv12, Plane};
    use crate::objc::{bytes_of, class, ns_string, string_of};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {}
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Dimensions {
        width: i32,
        height: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CMTime {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }

    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {
        fn CMVideoFormatDescriptionGetDimensions(description: *const c_void) -> Dimensions;
        fn CMSampleBufferGetImageBuffer(buffer: *const c_void) -> *const c_void;
        fn CMSampleBufferGetPresentationTimeStamp(buffer: *const c_void) -> CMTime;
    }

    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        fn CVPixelBufferLockBaseAddress(buffer: *const c_void, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(buffer: *const c_void, flags: u64) -> i32;
        fn CVPixelBufferGetWidth(buffer: *const c_void) -> usize;
        fn CVPixelBufferGetHeight(buffer: *const c_void) -> usize;
        fn CVPixelBufferGetPlaneCount(buffer: *const c_void) -> usize;
        fn CVPixelBufferGetBaseAddressOfPlane(buffer: *const c_void, plane: usize) -> *const u8;
        fn CVPixelBufferGetBytesPerRowOfPlane(buffer: *const c_void, plane: usize) -> usize;
        fn CVPixelBufferGetHeightOfPlane(buffer: *const c_void, plane: usize) -> usize;
    }

    extern "C" {
        fn dispatch_queue_create(label: *const i8, attributes: *const c_void) -> *mut AnyObject;
        fn dispatch_release(object: *mut AnyObject);
    }

    /// A `CMFormatDescriptionRef`, as the Objective-C runtime describes it.
    #[repr(C)]
    struct FormatDescription {
        _opaque: [u8; 0],
    }

    unsafe impl RefEncode for FormatDescription {
        const ENCODING_REF: Encoding =
            Encoding::Pointer(&Encoding::Struct("opaqueCMFormatDescription", &[]));
    }

    /// `AVMediaTypeVideo`.
    const MEDIA_VIDEO: &str = "vide";
    /// `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`, `420v`.
    const FORMAT_420V: u32 = 0x3432_3076;
    /// `kCVPixelBufferLock_ReadOnly`.
    const LOCK_READ_ONLY: u64 = 1;
    /// `NSBitmapImageFileTypeJPEG`.
    const JPEG: usize = 3;
    /// How long a running camera may go without a picture before it counts as stopped.
    const FRAME_WAIT: Duration = Duration::from_secs(3);

    struct Sink {
        frames: SyncSender<Frame>,
        fps: u32,
        last_ns: Option<u64>,
    }

    static SINKS: Mutex<Option<HashMap<usize, Sink>>> = Mutex::new(None);

    unsafe fn copy_frame(sample: *const c_void) -> Option<Frame> {
        let pixels = CMSampleBufferGetImageBuffer(sample);
        if pixels.is_null() || CVPixelBufferGetPlaneCount(pixels) != 2 {
            return None;
        }
        if CVPixelBufferLockBaseAddress(pixels, LOCK_READ_ONLY) != 0 {
            return None;
        }
        let width = CVPixelBufferGetWidth(pixels) as u32;
        let height = CVPixelBufferGetHeight(pixels) as u32;
        let plane = |index: usize| -> Option<Plane<'static>> {
            let base = CVPixelBufferGetBaseAddressOfPlane(pixels, index);
            if base.is_null() {
                return None;
            }
            let bytes_per_row = CVPixelBufferGetBytesPerRowOfPlane(pixels, index);
            let rows = CVPixelBufferGetHeightOfPlane(pixels, index);
            Some(Plane {
                bytes: std::slice::from_raw_parts(base, bytes_per_row * rows),
                bytes_per_row,
            })
        };
        let time = CMSampleBufferGetPresentationTimeStamp(sample);
        let timestamp_ns = if time.timescale > 0 && time.value >= 0 {
            (time.value as i128 * 1_000_000_000 / i128::from(time.timescale)) as u64
        } else {
            0
        };
        let frame = match (plane(0), plane(1)) {
            (Some(luma), Some(chroma)) => pack_nv12(width, height, luma, chroma, timestamp_ns),
            _ => None,
        };
        CVPixelBufferUnlockBaseAddress(pixels, LOCK_READ_ONLY);
        frame
    }

    unsafe extern "C-unwind" fn did_output(
        this: &AnyObject,
        _: Sel,
        _output: *mut AnyObject,
        sample: *const c_void,
        _connection: *mut AnyObject,
    ) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let key = this as *const AnyObject as usize;
            let Ok(mut sinks) = SINKS.lock() else { return };
            let Some(sink) = sinks.as_mut().and_then(|s| s.get_mut(&key)) else {
                return;
            };
            let Some(frame) = copy_frame(sample) else {
                return;
            };
            if let Some(last) = sink.last_ns {
                if !frame_due(frame.timestamp_ns.saturating_sub(last), sink.fps) {
                    return;
                }
            }
            sink.last_ns = Some(frame.timestamp_ns);
            // A guest that has not taken the last picture yet misses this one; the camera
            // never waits for it.
            if let Err(TrySendError::Disconnected(_)) = sink.frames.try_send(frame) {
                sinks.as_mut().map(|s| s.remove(&key));
            }
        }));
    }

    fn delegate_class() -> Result<&'static AnyClass, String> {
        static CLASS: OnceLock<Result<&'static AnyClass, String>> = OnceLock::new();
        CLASS
            .get_or_init(|| unsafe {
                let mut b = ClassBuilder::new(c"DSACameraDelegate", class(c"NSObject")?)
                    .ok_or("the camera delegate class is already defined")?;
                b.add_method(
                    sel!(captureOutput:didOutputSampleBuffer:fromConnection:),
                    did_output as unsafe extern "C-unwind" fn(_, _, _, _, _),
                );
                Ok(b.register())
            })
            .clone()
    }

    unsafe fn devices() -> Vec<*mut AnyObject> {
        let Ok(device_class) = class(c"AVCaptureDevice") else {
            return Vec::new();
        };
        let list: *mut AnyObject =
            msg_send![device_class, devicesWithMediaType: &*ns_string(MEDIA_VIDEO)];
        if list.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![list, count];
        (0..count)
            .map(|i| msg_send![list, objectAtIndex: i])
            .collect()
    }

    unsafe fn modes(device: *mut AnyObject) -> Vec<(Mode, *mut AnyObject)> {
        let formats: *mut AnyObject = msg_send![device, formats];
        if formats.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![formats, count];
        let mut out: Vec<(Mode, *mut AnyObject)> = Vec::new();
        for i in 0..count {
            let format: *mut AnyObject = msg_send![formats, objectAtIndex: i];
            let description: *const FormatDescription = msg_send![format, formatDescription];
            if description.is_null() {
                continue;
            }
            let size = CMVideoFormatDescriptionGetDimensions(description as *const c_void);
            let ranges: *mut AnyObject = msg_send![format, videoSupportedFrameRateRanges];
            let range_count: usize = if ranges.is_null() {
                0
            } else {
                msg_send![ranges, count]
            };
            let (mut min_fps, mut max_fps) = (u32::MAX, 0u32);
            for r in 0..range_count {
                let range: *mut AnyObject = msg_send![ranges, objectAtIndex: r];
                let min: f64 = msg_send![range, minFrameRate];
                let max: f64 = msg_send![range, maxFrameRate];
                min_fps = min_fps.min(min.floor().max(1.0) as u32);
                max_fps = max_fps.max(max.round() as u32);
            }
            if size.width <= 0 || size.height <= 0 || max_fps == 0 {
                continue;
            }
            let mode = Mode {
                width: size.width as u32,
                height: size.height as u32,
                min_fps,
                max_fps,
            };
            // A size offered in several pixel formats is listed once.
            if !out.iter().any(|(m, _)| *m == mode) {
                out.push((mode, format));
            }
        }
        out
    }

    /// The Mac's cameras.
    #[derive(Default)]
    pub struct MacCameras;

    impl MacCameras {
        pub fn new() -> MacCameras {
            MacCameras
        }
    }

    unsafe fn blocked() -> bool {
        let Ok(device_class) = class(c"AVCaptureDevice") else {
            return true;
        };
        let status: isize =
            msg_send![device_class, authorizationStatusForMediaType: &*ns_string(MEDIA_VIDEO)];
        // 1 restricted, 2 denied. Not asked yet (0) is not blocked: opening asks.
        matches!(status, 1 | 2)
    }

    impl Cameras for MacCameras {
        fn list(&self) -> Vec<Camera> {
            autoreleasepool(|_| unsafe {
                let blocked = blocked();
                devices()
                    .into_iter()
                    .map(|device| {
                        let position: isize = msg_send![device, position];
                        let device_type = string_of(msg_send![device, deviceType]);
                        Camera {
                            id: string_of(msg_send![device, uniqueID]),
                            name: string_of(msg_send![device, localizedName]),
                            facing: facing(position, &device_type),
                            sensor_orientation: 0,
                            modes: modes(device).into_iter().map(|(m, _)| m).collect(),
                            blocked,
                        }
                    })
                    .collect()
            })
        }

        fn open(&self, id: &str, mode: Mode, fps: u32) -> Result<Box<dyn CameraStream>, String> {
            // The first open asks the user, and waits for the answer.
            crate::devices::permissions::ask(crate::devices::permissions::Device::Camera)?;
            autoreleasepool(|_| unsafe { open(id, mode, fps) })
        }

        fn encode_jpeg(&self, frame: &Frame, quality: u8) -> Result<Vec<u8>, String> {
            if Frame::nv12_len(frame.width, frame.height, frame.stride) != Some(frame.nv12.len()) {
                return Err("the picture is not whole".into());
            }
            let mut rgb = nv12_to_rgb(frame);
            autoreleasepool(|_| unsafe {
                let rep_class = class(c"NSBitmapImageRep")?;
                let mut planes: [*mut std::ffi::c_char; 5] = [std::ptr::null_mut(); 5];
                planes[0] = rgb.as_mut_ptr() as *mut std::ffi::c_char;
                let rep: Allocated<AnyObject> = msg_send![rep_class, alloc];
                let rep: Option<Retained<AnyObject>> = msg_send![
                    rep,
                    initWithBitmapDataPlanes: planes.as_mut_ptr(),
                    pixelsWide: frame.width as isize,
                    pixelsHigh: frame.height as isize,
                    bitsPerSample: 8isize,
                    samplesPerPixel: 3isize,
                    hasAlpha: false,
                    isPlanar: false,
                    colorSpaceName: &*ns_string("NSDeviceRGBColorSpace"),
                    bytesPerRow: (frame.width * 3) as isize,
                    bitsPerPixel: 24isize
                ];
                let rep = rep.ok_or("the picture could not be wrapped for encoding")?;
                let number_class = class(c"NSNumber")?;
                let factor: *mut AnyObject = msg_send![
                    number_class,
                    numberWithFloat: f32::from(quality.clamp(1, 100)) / 100.0
                ];
                let dict_class = class(c"NSDictionary")?;
                let properties: *mut AnyObject = msg_send![
                    dict_class,
                    dictionaryWithObject: factor,
                    forKey: &*ns_string("NSImageCompressionFactor")
                ];
                let data: *mut AnyObject = msg_send![
                    &*rep,
                    representationUsingType: JPEG,
                    properties: properties
                ];
                let jpeg = bytes_of(data);
                if jpeg.is_empty() {
                    Err("the picture could not be encoded as JPEG".to_string())
                } else {
                    Ok(jpeg)
                }
            })
        }
    }

    unsafe fn open(id: &str, mode: Mode, fps: u32) -> Result<Box<dyn CameraStream>, String> {
        let device_class = class(c"AVCaptureDevice")?;
        let device: *mut AnyObject = msg_send![device_class, deviceWithUniqueID: &*ns_string(id)];
        if device.is_null() {
            return Err(format!("the camera {id} is no longer connected"));
        }
        let format = modes(device)
            .into_iter()
            .find(|(m, _)| m.width == mode.width && m.height == mode.height)
            .map(|(_, f)| f)
            .ok_or_else(|| format!("the camera does not offer {}x{}", mode.width, mode.height))?;

        let session: Option<Retained<AnyObject>> = msg_send![class(c"AVCaptureSession")?, new];
        let session = session.ok_or("no capture session could be made")?;
        let _: () = msg_send![&*session, beginConfiguration];
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let input: *mut AnyObject = msg_send![
            class(c"AVCaptureDeviceInput")?,
            deviceInputWithDevice: device,
            error: &mut error
        ];
        if input.is_null() {
            return Err(
                "the camera could not be opened: the user may not have allowed DSA to use it, \
                 in System Settings, Privacy & Security, Camera"
                    .into(),
            );
        }
        let can_add: bool = msg_send![&*session, canAddInput: input];
        if !can_add {
            return Err("the camera is in use by another program".into());
        }
        let _: () = msg_send![&*session, addInput: input];

        let output: Option<Retained<AnyObject>> =
            msg_send![class(c"AVCaptureVideoDataOutput")?, new];
        let output = output.ok_or("no video output could be made")?;
        let number: *mut AnyObject =
            msg_send![class(c"NSNumber")?, numberWithUnsignedInt: FORMAT_420V];
        let settings: *mut AnyObject = msg_send![
            class(c"NSDictionary")?,
            dictionaryWithObject: number,
            forKey: &*ns_string("PixelFormatType")
        ];
        let _: () = msg_send![&*output, setVideoSettings: settings];
        let _: () = msg_send![&*output, setAlwaysDiscardsLateVideoFrames: true];
        let delegate: Option<Retained<AnyObject>> = msg_send![delegate_class()?, new];
        let delegate = delegate.ok_or("no camera delegate could be made")?;
        let queue = dispatch_queue_create(c"org.thisisthepy.dsa.camera".as_ptr(), std::ptr::null());
        let _: () = msg_send![&*output, setSampleBufferDelegate: &*delegate, queue: queue];
        let can_add: bool = msg_send![&*session, canAddOutput: &*output];
        if !can_add {
            dispatch_release(queue);
            return Err("the camera's pictures cannot be read".into());
        }
        let _: () = msg_send![&*session, addOutput: &*output];

        let mut error: *mut AnyObject = std::ptr::null_mut();
        let locked: bool = msg_send![device, lockForConfiguration: &mut error];
        if locked {
            let _: () = msg_send![device, setActiveFormat: format];
        }
        let _: () = msg_send![&*session, commitConfiguration];
        if locked {
            let _: () = msg_send![device, unlockForConfiguration];
        }

        let (frames, receiver) = sync_channel(2);
        let key = Retained::as_ptr(&delegate) as usize;
        SINKS
            .lock()
            .map_err(|_| "the camera lock failed")?
            .get_or_insert_with(HashMap::new)
            .insert(
                key,
                Sink {
                    frames,
                    fps,
                    last_ns: None,
                },
            );
        let _: () = msg_send![&*session, startRunning];
        Ok(Box::new(Stream {
            session,
            output,
            delegate,
            queue,
            frames: receiver,
        }))
    }

    struct Stream {
        session: Retained<AnyObject>,
        output: Retained<AnyObject>,
        delegate: Retained<AnyObject>,
        queue: *mut AnyObject,
        frames: Receiver<Frame>,
    }

    // The session and its output may be stopped from any thread.
    unsafe impl Send for Stream {}

    impl CameraStream for Stream {
        fn next(&mut self) -> Result<Frame, String> {
            self.frames
                .recv_timeout(FRAME_WAIT)
                .map_err(|_| "the camera stopped sending pictures".to_string())
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![&*self.session, stopRunning];
                let _: () = msg_send![
                    &*self.output,
                    setSampleBufferDelegate: std::ptr::null::<AnyObject>(),
                    queue: std::ptr::null::<AnyObject>()
                ];
                if let Ok(mut sinks) = SINKS.lock() {
                    if let Some(sinks) = sinks.as_mut() {
                        sinks.remove(&(Retained::as_ptr(&self.delegate) as usize));
                    }
                }
                dispatch_release(self.queue);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr26_a_macs_built_in_camera_faces_the_user() {
        assert_eq!(
            facing(0, "AVCaptureDeviceTypeBuiltInWideAngleCamera"),
            Facing::Front
        );
        assert_eq!(facing(0, "AVCaptureDeviceTypeExternal"), Facing::External);
        assert_eq!(
            facing(0, "AVCaptureDeviceTypeContinuityCamera"),
            Facing::External
        );
        assert_eq!(facing(1, ""), Facing::Back);
    }

    #[test]
    fn fr26_both_planes_are_copied_into_one_nv12_picture() {
        // 3x2 with padded rows: luma rows of 8 bytes, one chroma row of 8.
        let luma: Vec<u8> = (0..16).collect();
        let chroma: Vec<u8> = (100..108).collect();
        let frame = pack_nv12(
            3,
            2,
            Plane {
                bytes: &luma,
                bytes_per_row: 8,
            },
            Plane {
                bytes: &chroma,
                bytes_per_row: 8,
            },
            42,
        )
        .unwrap();
        assert_eq!(frame.stride, 4);
        assert_eq!(frame.nv12.len(), 4 * 3);
        assert_eq!(&frame.nv12[..3], &[0, 1, 2]);
        assert_eq!(&frame.nv12[4..7], &[8, 9, 10]);
        assert_eq!(&frame.nv12[8..12], &[100, 101, 102, 103]);
        assert_eq!(frame.timestamp_ns, 42);
    }

    #[test]
    fn fr26_a_plane_shorter_than_its_size_is_refused() {
        let short = [0u8; 4];
        let plane = Plane {
            bytes: &short,
            bytes_per_row: 4,
        };
        assert!(pack_nv12(4, 4, plane, plane, 0).is_none());
    }

    #[test]
    fn fr26_grey_stays_grey_in_rgb() {
        let frame = Frame {
            width: 2,
            height: 2,
            stride: 2,
            nv12: vec![126, 126, 126, 126, 128, 128],
            timestamp_ns: 0,
        };
        let rgb = nv12_to_rgb(&frame);
        assert_eq!(rgb.len(), 12);
        assert!(rgb.iter().all(|v| *v == 128));
    }

    #[test]
    fn fr26_frames_beyond_the_rate_asked_for_are_dropped() {
        assert!(frame_due(33_333_333, 30));
        assert!(frame_due(30_000_000, 30));
        assert!(!frame_due(16_666_666, 30));
        assert!(frame_due(1, 0));
    }
}
