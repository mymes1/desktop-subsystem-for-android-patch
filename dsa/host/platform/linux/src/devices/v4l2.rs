//! The parts of Video4Linux2 the cameras use: the structures and requests of
//! `linux/videodev2.h`, and a capture device with memory-mapped buffers.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

/// `V4L2_BUF_TYPE_VIDEO_CAPTURE`.
pub const BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
/// `V4L2_MEMORY_MMAP`.
pub const MEMORY_MMAP: u32 = 1;
/// `V4L2_FIELD_ANY`.
const FIELD_ANY: u32 = 0;
/// `V4L2_CAP_VIDEO_CAPTURE`.
pub const CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
/// `V4L2_CAP_STREAMING`.
pub const CAP_STREAMING: u32 = 0x0400_0000;
/// `V4L2_CAP_DEVICE_CAPS`.
pub const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
/// `V4L2_FRMSIZE_TYPE_DISCRETE`.
pub const FRMSIZE_TYPE_DISCRETE: u32 = 1;
/// `V4L2_FRMIVAL_TYPE_DISCRETE`.
pub const FRMIVAL_TYPE_DISCRETE: u32 = 1;
/// `V4L2_CID_CAMERA_ORIENTATION`: where the camera faces.
pub const CID_CAMERA_ORIENTATION: u32 = 0x009a_0922;
/// `V4L2_CID_CAMERA_SENSOR_ROTATION`: how the sensor is turned in the enclosure.
pub const CID_CAMERA_SENSOR_ROTATION: u32 = 0x009a_0923;

/// A pixel format's four characters as V4L2 numbers them.
pub const fn fourcc(code: &[u8; 4]) -> u32 {
    (code[0] as u32) | (code[1] as u32) << 8 | (code[2] as u32) << 16 | (code[3] as u32) << 24
}

pub const PIX_YUYV: u32 = fourcc(b"YUYV");
pub const PIX_NV12: u32 = fourcc(b"NV12");
pub const PIX_YU12: u32 = fourcc(b"YU12");
pub const PIX_MJPEG: u32 = fourcc(b"MJPG");
pub const PIX_JPEG: u32 = fourcc(b"JPEG");

#[repr(C)]
pub struct Capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
pub struct FmtDesc {
    pub index: u32,
    pub kind: u32,
    pub flags: u32,
    pub description: [u8; 32],
    pub pixelformat: u32,
    pub mbus_code: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
pub struct FrmSizeEnum {
    pub index: u32,
    pub pixel_format: u32,
    pub kind: u32,
    /// The discrete size (width, height), or the stepwise range (min width, max width, step,
    /// min height, max height, step).
    pub size: [u32; 6],
    pub reserved: [u32; 2],
}

#[repr(C)]
pub struct FrmIvalEnum {
    pub index: u32,
    pub pixel_format: u32,
    pub width: u32,
    pub height: u32,
    pub kind: u32,
    /// The discrete interval (numerator, denominator), or the stepwise range (min, max,
    /// step), each a fraction.
    pub interval: [u32; 6],
    pub reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PixFormat {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub bytesperline: u32,
    pub sizeimage: u32,
    pub colorspace: u32,
    pub private: u32,
    pub flags: u32,
    pub ycbcr_enc: u32,
    pub quantization: u32,
    pub xfer_func: u32,
}

#[repr(C)]
pub union FormatUnion {
    pub pix: PixFormat,
    pub raw: [u8; 200],
    /// The union holds pointers in some of its members, so it is aligned as one.
    _align: [*mut c_void; 0],
}

#[repr(C)]
pub struct Format {
    pub kind: u32,
    pub fmt: FormatUnion,
}

#[repr(C)]
pub struct RequestBuffers {
    pub count: u32,
    pub kind: u32,
    pub memory: u32,
    pub capabilities: u32,
    pub flags: u8,
    pub reserved: [u8; 3],
}

#[repr(C)]
pub struct Timecode {
    pub kind: u32,
    pub flags: u32,
    pub frames: u8,
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub userbits: [u8; 4],
}

#[repr(C)]
pub union BufferM {
    pub offset: u32,
    pub userptr: libc::c_ulong,
    pub planes: *mut c_void,
    pub fd: i32,
}

#[repr(C)]
pub struct Buffer {
    pub index: u32,
    pub kind: u32,
    pub bytesused: u32,
    pub flags: u32,
    pub field: u32,
    pub timestamp: libc::timeval,
    pub timecode: Timecode,
    pub sequence: u32,
    pub memory: u32,
    pub m: BufferM,
    pub length: u32,
    pub reserved2: u32,
    pub request_fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CaptureParm {
    pub capability: u32,
    pub capturemode: u32,
    pub timeperframe: [u32; 2],
    pub extendedmode: u32,
    pub readbuffers: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
pub union ParmUnion {
    pub capture: CaptureParm,
    pub raw: [u8; 200],
}

#[repr(C)]
pub struct StreamParm {
    pub kind: u32,
    pub parm: ParmUnion,
}

#[repr(C)]
pub struct Control {
    pub id: u32,
    pub value: i32,
}

const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc(dir: u32, nr: u32, size: usize) -> libc::c_ulong {
    ((dir << 30) | ((size as u32) << 16) | ((b'V' as u32) << 8) | nr) as libc::c_ulong
}

pub const VIDIOC_QUERYCAP: libc::c_ulong = ioc(IOC_READ, 0, size_of::<Capability>());
pub const VIDIOC_ENUM_FMT: libc::c_ulong = ioc(IOC_READ | IOC_WRITE, 2, size_of::<FmtDesc>());
pub const VIDIOC_S_FMT: libc::c_ulong = ioc(IOC_READ | IOC_WRITE, 5, size_of::<Format>());
pub const VIDIOC_REQBUFS: libc::c_ulong =
    ioc(IOC_READ | IOC_WRITE, 8, size_of::<RequestBuffers>());
pub const VIDIOC_QUERYBUF: libc::c_ulong = ioc(IOC_READ | IOC_WRITE, 9, size_of::<Buffer>());
pub const VIDIOC_QBUF: libc::c_ulong = ioc(IOC_READ | IOC_WRITE, 15, size_of::<Buffer>());
pub const VIDIOC_DQBUF: libc::c_ulong = ioc(IOC_READ | IOC_WRITE, 17, size_of::<Buffer>());
pub const VIDIOC_STREAMON: libc::c_ulong = ioc(IOC_WRITE, 18, size_of::<libc::c_int>());
pub const VIDIOC_STREAMOFF: libc::c_ulong = ioc(IOC_WRITE, 19, size_of::<libc::c_int>());
pub const VIDIOC_S_PARM: libc::c_ulong = ioc(IOC_READ | IOC_WRITE, 22, size_of::<StreamParm>());
pub const VIDIOC_G_CTRL: libc::c_ulong = ioc(IOC_READ | IOC_WRITE, 27, size_of::<Control>());
pub const VIDIOC_ENUM_FRAMESIZES: libc::c_ulong =
    ioc(IOC_READ | IOC_WRITE, 74, size_of::<FrmSizeEnum>());
pub const VIDIOC_ENUM_FRAMEINTERVALS: libc::c_ulong =
    ioc(IOC_READ | IOC_WRITE, 75, size_of::<FrmIvalEnum>());

/// Makes a V4L2 request on `fd`, again while a signal interrupts it.
///
/// # Safety
///
/// `arg` must be the structure `request` takes.
pub unsafe fn xioctl<T>(fd: libc::c_int, request: libc::c_ulong, arg: *mut T) -> io::Result<()> {
    loop {
        // SAFETY: the caller passes the structure the request takes. The request's type is
        // c_ulong on glibc and c_int on musl; `as _` takes either.
        let result = unsafe { libc::ioctl(fd, request as _, arg) };
        if result >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// A plain structure of the API, zeroed.
pub fn zeroed_value<T>() -> T {
    // SAFETY: every structure here is plain data, for which all zero bytes is valid.
    unsafe { zeroed() }
}

/// A C string from a fixed array of the API.
pub fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// An open V4L2 device.
pub struct Device {
    pub file: File,
}

impl Device {
    pub fn open(path: &Path) -> io::Result<Device> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        Ok(Device { file })
    }

    pub fn fd(&self) -> libc::c_int {
        self.file.as_raw_fd()
    }

    pub fn capability(&self) -> io::Result<Capability> {
        let mut cap: Capability = zeroed_value();
        // SAFETY: VIDIOC_QUERYCAP takes a v4l2_capability.
        unsafe { xioctl(self.fd(), VIDIOC_QUERYCAP, &mut cap) }?;
        Ok(cap)
    }

    /// The pixel formats the device captures in.
    pub fn formats(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for index in 0..64 {
            let mut desc: FmtDesc = zeroed_value();
            desc.index = index;
            desc.kind = BUF_TYPE_VIDEO_CAPTURE;
            // SAFETY: VIDIOC_ENUM_FMT takes a v4l2_fmtdesc.
            if unsafe { xioctl(self.fd(), VIDIOC_ENUM_FMT, &mut desc) }.is_err() {
                break;
            }
            out.push(desc.pixelformat);
        }
        out
    }

    /// The discrete sizes the device captures `format` at. A device with a stepwise range
    /// offers its largest size and the common sizes inside the range.
    pub fn sizes(&self, format: u32) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for index in 0..128 {
            let mut size: FrmSizeEnum = zeroed_value();
            size.index = index;
            size.pixel_format = format;
            // SAFETY: VIDIOC_ENUM_FRAMESIZES takes a v4l2_frmsizeenum.
            if unsafe { xioctl(self.fd(), VIDIOC_ENUM_FRAMESIZES, &mut size) }.is_err() {
                break;
            }
            if size.kind == FRMSIZE_TYPE_DISCRETE {
                out.push((size.size[0], size.size[1]));
            } else {
                let [min_w, max_w, _, min_h, max_h, _] = size.size;
                for (w, h) in COMMON_SIZES {
                    if (min_w..=max_w).contains(&w) && (min_h..=max_h).contains(&h) {
                        out.push((w, h));
                    }
                }
                out.push((max_w, max_h));
                break;
            }
        }
        out
    }

    /// The frame rates the device captures `format` at `width` by `height`, as (slowest,
    /// fastest) in whole frames a second.
    pub fn rates(&self, format: u32, width: u32, height: u32) -> Option<(u32, u32)> {
        let mut rates: Vec<f64> = Vec::new();
        for index in 0..64 {
            let mut ival: FrmIvalEnum = zeroed_value();
            ival.index = index;
            ival.pixel_format = format;
            ival.width = width;
            ival.height = height;
            // SAFETY: VIDIOC_ENUM_FRAMEINTERVALS takes a v4l2_frmivalenum.
            if unsafe { xioctl(self.fd(), VIDIOC_ENUM_FRAMEINTERVALS, &mut ival) }.is_err() {
                break;
            }
            let fps = |num: u32, den: u32| (num > 0).then(|| f64::from(den) / f64::from(num));
            if ival.kind == FRMIVAL_TYPE_DISCRETE {
                rates.extend(fps(ival.interval[0], ival.interval[1]));
            } else {
                // The shortest interval is the fastest rate.
                rates.extend(fps(ival.interval[0], ival.interval[1]));
                rates.extend(fps(ival.interval[2], ival.interval[3]));
                break;
            }
        }
        let rates: Vec<u32> = rates
            .into_iter()
            .filter(|r| r.is_finite() && *r >= 0.5)
            .map(|r| r.round() as u32)
            .collect();
        match (rates.iter().min(), rates.iter().max()) {
            (Some(&min), Some(&max)) => Some((min, max)),
            // A device that does not list its rates is taken at 30.
            _ => Some((30, 30)),
        }
    }

    /// A control's value, if the device has it.
    pub fn control(&self, id: u32) -> Option<i32> {
        let mut control = Control { id, value: 0 };
        // SAFETY: VIDIOC_G_CTRL takes a v4l2_control.
        unsafe { xioctl(self.fd(), VIDIOC_G_CTRL, &mut control) }.ok()?;
        Some(control.value)
    }
}

/// Sizes a device with a stepwise range is offered at.
const COMMON_SIZES: [(u32, u32); 6] = [
    (320, 240),
    (640, 480),
    (1280, 720),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
];

/// One buffer the driver fills, mapped into this process.
struct Mapped {
    address: *mut c_void,
    length: usize,
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: a mapping made by mmap with this length, unmapped once.
        unsafe { libc::munmap(self.address, self.length) };
    }
}

/// A device that is capturing.
pub struct Capture {
    device: Device,
    buffers: Vec<Mapped>,
    pub format: PixFormat,
}

// SAFETY: the mappings belong to this capture alone, and it is used by one thread at a time.
unsafe impl Send for Capture {}

impl Capture {
    /// Starts `device` capturing `format` at `width` by `height` and `fps` frames a second.
    pub fn start(
        device: Device,
        format: u32,
        width: u32,
        height: u32,
        fps: u32,
    ) -> io::Result<Capture> {
        let fd = device.fd();
        let mut fmt: Format = zeroed_value();
        fmt.kind = BUF_TYPE_VIDEO_CAPTURE;
        // SAFETY: writing the pix member of a zeroed union.
        unsafe {
            fmt.fmt.pix.width = width;
            fmt.fmt.pix.height = height;
            fmt.fmt.pix.pixelformat = format;
            fmt.fmt.pix.field = FIELD_ANY;
        }
        // SAFETY: VIDIOC_S_FMT takes a v4l2_format.
        unsafe { xioctl(fd, VIDIOC_S_FMT, &mut fmt) }?;
        // SAFETY: the driver filled the pix member.
        let chosen = unsafe { fmt.fmt.pix };
        if chosen.pixelformat != format {
            return Err(io::Error::other("the camera did not take the pixel format"));
        }

        if fps > 0 {
            let mut parm: StreamParm = zeroed_value();
            parm.kind = BUF_TYPE_VIDEO_CAPTURE;
            // SAFETY: writing the capture member of a zeroed union.
            unsafe { parm.parm.capture.timeperframe = [1, fps] };
            // SAFETY: VIDIOC_S_PARM takes a v4l2_streamparm. A device that cannot set its
            // rate runs at its own.
            let _ = unsafe { xioctl(fd, VIDIOC_S_PARM, &mut parm) };
        }

        let mut request = RequestBuffers {
            count: 4,
            kind: BUF_TYPE_VIDEO_CAPTURE,
            memory: MEMORY_MMAP,
            capabilities: 0,
            flags: 0,
            reserved: [0; 3],
        };
        // SAFETY: VIDIOC_REQBUFS takes a v4l2_requestbuffers.
        unsafe { xioctl(fd, VIDIOC_REQBUFS, &mut request) }?;
        if request.count == 0 {
            return Err(io::Error::other("the camera gave no buffers"));
        }
        let mut buffers = Vec::new();
        for index in 0..request.count {
            let mut buffer: Buffer = zeroed_value();
            buffer.kind = BUF_TYPE_VIDEO_CAPTURE;
            buffer.memory = MEMORY_MMAP;
            buffer.index = index;
            // SAFETY: VIDIOC_QUERYBUF takes a v4l2_buffer.
            unsafe { xioctl(fd, VIDIOC_QUERYBUF, &mut buffer) }?;
            let length = buffer.length as usize;
            // SAFETY: the driver's offset for this buffer; the mapping is checked.
            let address = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    buffer.m.offset as libc::off_t,
                )
            };
            if address == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            buffers.push(Mapped { address, length });
            // SAFETY: VIDIOC_QBUF takes a v4l2_buffer.
            unsafe { xioctl(fd, VIDIOC_QBUF, &mut buffer) }?;
        }
        let mut kind: libc::c_int = BUF_TYPE_VIDEO_CAPTURE as libc::c_int;
        // SAFETY: VIDIOC_STREAMON takes the buffer type.
        unsafe { xioctl(fd, VIDIOC_STREAMON, &mut kind) }?;
        Ok(Capture {
            device,
            buffers,
            format: chosen,
        })
    }

    /// Waits at most `timeout` for the next picture and hands its bytes and its time in
    /// nanoseconds to `take`.
    pub fn next<R>(
        &mut self,
        timeout: Duration,
        take: impl FnOnce(&[u8], u64) -> R,
    ) -> io::Result<R> {
        let fd = self.device.fd();
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        if ready == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the camera sent no picture",
            ));
        }
        let mut buffer: Buffer = zeroed_value();
        buffer.kind = BUF_TYPE_VIDEO_CAPTURE;
        buffer.memory = MEMORY_MMAP;
        // SAFETY: VIDIOC_DQBUF takes a v4l2_buffer.
        unsafe { xioctl(fd, VIDIOC_DQBUF, &mut buffer) }?;
        let index = buffer.index as usize;
        let result = match self.buffers.get(index) {
            Some(mapped) => {
                let used = (buffer.bytesused as usize).min(mapped.length);
                // SAFETY: the driver filled `used` bytes of this mapping, and gives it back
                // only once it is queued again below.
                let bytes =
                    unsafe { std::slice::from_raw_parts(mapped.address as *const u8, used) };
                let nanos = (buffer.timestamp.tv_sec as u64)
                    .saturating_mul(1_000_000_000)
                    .saturating_add(buffer.timestamp.tv_usec as u64 * 1000);
                Ok(take(bytes, nanos))
            }
            None => Err(io::Error::other("the camera named a buffer it does not have")),
        };
        // SAFETY: VIDIOC_QBUF takes a v4l2_buffer; the same one goes back to the driver.
        unsafe { xioctl(fd, VIDIOC_QBUF, &mut buffer) }?;
        result
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let mut kind: libc::c_int = BUF_TYPE_VIDEO_CAPTURE as libc::c_int;
        // SAFETY: VIDIOC_STREAMOFF takes the buffer type.
        let _ = unsafe { xioctl(self.device.fd(), VIDIOC_STREAMOFF, &mut kind) };
        self.buffers.clear();
    }
}

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use super::*;

    #[test]
    fn fr26_the_v4l2_structures_have_the_kernels_layout() {
        assert_eq!(size_of::<Capability>(), 104);
        assert_eq!(size_of::<FmtDesc>(), 64);
        assert_eq!(size_of::<FrmSizeEnum>(), 44);
        assert_eq!(size_of::<FrmIvalEnum>(), 52);
        assert_eq!(size_of::<Format>(), 208);
        assert_eq!(size_of::<RequestBuffers>(), 20);
        assert_eq!(size_of::<Buffer>(), 88);
        assert_eq!(size_of::<StreamParm>(), 204);
        assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(VIDIOC_S_FMT, 0xc0d0_5605);
        assert_eq!(VIDIOC_DQBUF, 0xc058_5611);
        assert_eq!(VIDIOC_STREAMON, 0x4004_5612);
    }

    #[test]
    fn fr26_pixel_formats_are_named_as_v4l2_names_them() {
        assert_eq!(PIX_YUYV, 0x5659_5559);
        assert_eq!(PIX_MJPEG, 0x4750_4a4d);
    }
}
