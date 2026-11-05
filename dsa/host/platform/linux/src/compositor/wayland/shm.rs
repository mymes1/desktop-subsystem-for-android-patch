//! Shared memory buffers (`wl_shm`) that a window's frames are copied into.

use std::io;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use wayland_client::protocol::{wl_buffer::WlBuffer, wl_shm, wl_shm_pool::WlShmPool};
use wayland_client::QueueHandle;

use super::State;

/// One `XRGB8888` buffer, mapped into this process.
pub struct ShmBuffer {
    pub buffer: WlBuffer,
    pool: WlShmPool,
    map: *mut u8,
    len: usize,
    pub size: (u32, u32),
    /// Whether the compositor still reads the buffer (between attach and `release`).
    pub busy: Arc<AtomicBool>,
    _fd: OwnedFd,
}

impl ShmBuffer {
    pub fn new(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<State>,
        size: (u32, u32),
    ) -> io::Result<ShmBuffer> {
        let (width, height) = (size.0.max(1), size.1.max(1));
        let stride = width as usize * 4;
        let len = stride * height as usize;
        // SAFETY: a new anonymous file, owned from here on.
        let fd = unsafe {
            let raw = libc::memfd_create(c"dsa-frame".as_ptr(), libc::MFD_CLOEXEC);
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(raw)
        };
        // SAFETY: sizes the file just made, then maps all of it.
        let map = unsafe {
            use std::os::fd::AsRawFd;
            if libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) < 0 {
                return Err(io::Error::last_os_error());
            }
            let map = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            if map == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            map.cast::<u8>()
        };
        let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
        let busy = Arc::new(AtomicBool::new(false));
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_shm::Format::Xrgb8888,
            qh,
            busy.clone(),
        );
        Ok(ShmBuffer {
            buffer,
            pool,
            map,
            len,
            size: (width, height),
            busy,
            _fd: fd,
        })
    }

    /// Copies `pixels` (`XRGB8888`, as large as the buffer) in.
    pub fn write(&mut self, pixels: &[u8]) {
        let len = pixels.len().min(self.len);
        // SAFETY: the map is `self.len` bytes and stays mapped while `self` lives; the
        // compositor does not read the buffer while it is not busy.
        unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), self.map, len) };
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    pub fn set_busy(&self) {
        self.busy.store(true, Ordering::Release);
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        // SAFETY: unmaps what `new` mapped, once.
        unsafe {
            libc::munmap(self.map.cast(), self.len);
        }
    }
}
