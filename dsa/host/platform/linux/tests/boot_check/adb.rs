//! adb against the booted guest, through the service's developer mode port, and the screen
//! read through it: `screencap`, and a `screenrecord` frame when the capture comes back
//! empty. The boot checks that read what Android drew share it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dsa_host_core::apk_info::inflate::inflate;

use super::{run_bounded, Bounded};

/// How long one adb command may take. A dump or a capture takes seconds.
pub const ADB_LIMIT: Duration = Duration::from_secs(120);
/// How long the screenrecord fallback may take: it records for 3 s.
pub const SCREENRECORD_LIMIT: Duration = Duration::from_secs(30);
/// How many frames the screenrecord fallback reads before it stops: the newest of them is
/// kept, and raw frames are large (6 MiB at 1920x1080).
pub const SCREENRECORD_FRAMES: usize = 4;

/// Whether a picture is nothing but zeros: a capture that never received the screen, since
/// a drawn screen is opaque.
pub fn empty_picture(pixels: &[u8]) -> bool {
    pixels.iter().all(|&byte| byte == 0)
}

/// Whether an RGBA picture is one colour from edge to edge: all zeros when the GPU's drawing
/// never reached the capture's memory, and never a screen with something drawn on it.
pub fn blank_picture(pixels: &[u8]) -> bool {
    match pixels.get(..4) {
        Some(first) => pixels.chunks_exact(4).all(|pixel| pixel == first),
        None => true,
    }
}

/// Why the captures in `names` fail the check.
pub fn blank_screencaps_failure(names: &[String]) -> String {
    format!(
        "screencap gave a blank picture of {}: the GPU's drawing did not come back to the memory screencap reads",
        names.join(", ")
    )
}

/// The newest whole frame of `screenrecord --output-format=raw-frames`, which writes each
/// frame as `width` x `height` RGB pixels and nothing between them, as opaque RGBA. `None`
/// when not even one whole frame came.
pub fn newest_raw_frame(frames: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    let size = width * height * 3;
    if size == 0 || frames.len() < size {
        return None;
    }
    let start = (frames.len() / size - 1) * size;
    Some(
        frames[start..start + size]
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect(),
    )
}

/// The pixels of a PNG with 8 bits per sample, as `(width, height, channels, rows)`.
pub fn decode_png(png: &[u8]) -> Result<(usize, usize, usize, Vec<u8>), String> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if png.len() < 8 || png[..8] != SIGNATURE {
        return Err("not a PNG".into());
    }
    let (mut at, mut header, mut data) = (8, None, Vec::new());
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        let kind = &png[at + 4..at + 8];
        let body = png
            .get(at + 8..at + 8 + len)
            .ok_or("a PNG chunk runs past the end")?;
        match kind {
            b"IHDR" => header = Some(body.to_vec()),
            b"IDAT" => data.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    let header = header.ok_or("the PNG has no header")?;
    let width = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
    let height = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
    let (depth, colour, interlace) = (header[8], header[9], header[12]);
    let channels = match colour {
        2 => 3,
        6 => 4,
        other => {
            return Err(format!(
                "the PNG has colour type {other}, which is not checked"
            ))
        }
    };
    if depth != 8 || interlace != 0 {
        return Err(format!(
            "the PNG has {depth} bits per sample and interlace {interlace}; 8 and none are checked"
        ));
    }
    if data.len() < 2 {
        return Err("the PNG has no image data".into());
    }
    let stride = width * channels;
    // A zlib stream: two bytes of header before the DEFLATE data.
    let raw = inflate(&data[2..], height * (stride + 1))?;
    if raw.len() < height * (stride + 1) {
        return Err("the PNG's image data is short".into());
    }
    let mut out = vec![0u8; height * stride];
    for y in 0..height {
        let filter = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for x in 0..stride {
            let a = if x >= channels {
                out[y * stride + x - channels]
            } else {
                0
            };
            let b = if y > 0 { out[(y - 1) * stride + x] } else { 0 };
            let c = if x >= channels && y > 0 {
                out[(y - 1) * stride + x - channels]
            } else {
                0
            };
            let predicted = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => {
                    let p = a as i16 + b as i16 - c as i16;
                    let pa = (p - a as i16).abs();
                    let pb = (p - b as i16).abs();
                    let pc = (p - c as i16).abs();
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                other => return Err(format!("the PNG uses filter {other}")),
            };
            out[y * stride + x] = line[x].wrapping_add(predicted);
        }
    }
    // RGBA, whatever the PNG carried.
    if channels == 3 {
        out = out
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
    }
    Ok((width, height, 4, out))
}

/// adb, talking to the guest through the service's developer mode port.
pub struct Adb {
    pub program: PathBuf,
    pub serial: String,
    /// The captures screencap gave blank, by name.
    pub blank_screencaps: RefCell<Vec<String>>,
}

impl Adb {
    /// Runs adb with `args` against the guest, for [ADB_LIMIT] at most: a command that
    /// hangs fails the check with what it was, instead of holding it until the job's timeout.
    pub fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let out = run_bounded(
            Command::new(&self.program)
                .arg("-s")
                .arg(&self.serial)
                .args(args),
            ADB_LIMIT,
            usize::MAX,
        )?;
        match out.status {
            Some(status) if status.success() => Ok(out.stdout),
            Some(_) => Err(format!(
                "adb {} failed: {}{}",
                args.join(" "),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )),
            None => Err(format!(
                "adb {} did not finish within {} s and was stopped",
                args.join(" "),
                ADB_LIMIT.as_secs()
            )),
        }
    }

    pub fn shell(&self, command: &str) -> Result<String, String> {
        self.run(&["shell", command])
            .map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    /// Connects, and waits until the guest's shell answers.
    pub fn connect(&self, wait: Duration) -> Result<(), String> {
        let deadline = Instant::now() + wait;
        let _ = run_bounded(
            Command::new(&self.program).arg("start-server"),
            ADB_LIMIT,
            usize::MAX,
        );
        loop {
            let _ = run_bounded(
                Command::new(&self.program).arg("connect").arg(&self.serial),
                ADB_LIMIT,
                usize::MAX,
            );
            match self.shell("echo ready") {
                Ok(out) if out.trim() == "ready" => return Ok(()),
                Ok(out) if Instant::now() >= deadline => {
                    return Err(format!("the guest's shell answered {out:?}"))
                }
                Err(why) if Instant::now() >= deadline => return Err(why),
                _ => std::thread::sleep(Duration::from_secs(2)),
            }
        }
    }

    /// The screen, kept in `dir` as `name.png`, as `(width, RGBA pixels)`.
    ///
    /// `screencap` maps SurfaceFlinger's capture for the CPU, and the guest driver copies
    /// the GPU's drawing into it from the host. A capture that comes back blank is noted in
    /// [Adb::blank_screencaps], for the check to fail on. Only then, and only so the other
    /// checks still run, the screen is read again from a `screenrecord` frame, which the GPU
    /// reads back itself (`glReadPixels`), and that picture is kept as `name-record.png`.
    /// screenrecord runs for [SCREENRECORD_LIMIT] at most, and stops once it has sent
    /// [SCREENRECORD_FRAMES] frames: a screenrecord that sends nothing held the whole check
    /// until the job's timeout.
    pub fn screen(&self, dir: &Path, name: &str) -> Result<(usize, Vec<u8>), String> {
        let png = self.run(&["exec-out", "screencap", "-p"])?;
        let _ = std::fs::write(dir.join(format!("{name}.png")), &png);
        let (width, height, _, pixels) = decode_png(&png)?;
        if blank_picture(&pixels) {
            self.blank_screencaps.borrow_mut().push(name.to_string());
        }
        if !empty_picture(&pixels) {
            return Ok((width, pixels));
        }
        eprintln!(
            "screencap gave an empty picture of {name} ({width}x{height}); reading a screenrecord frame instead"
        );
        let pixels = self.recorded_screen(dir, name, width, height)?;
        if empty_picture(&pixels) {
            return Err(format!(
                "both screencap and screenrecord gave an empty picture of {name}"
            ));
        }
        Ok((width, pixels))
    }

    /// The screen, `width` x `height`, read from the newest whole `screenrecord` frame and
    /// kept in `dir` as `name-record.png`, as RGBA pixels. See [Adb::screen].
    pub fn recorded_screen(
        &self,
        dir: &Path,
        name: &str,
        width: usize,
        height: usize,
    ) -> Result<Vec<u8>, String> {
        let size = format!("{width}x{height}");
        let recorded = run_bounded(
            Command::new(&self.program)
                .arg("-s")
                .arg(&self.serial)
                .args([
                    "exec-out",
                    "screenrecord",
                    "--output-format=raw-frames",
                    "--size",
                    size.as_str(),
                    "--time-limit",
                    "3",
                    "-",
                ]),
            SCREENRECORD_LIMIT,
            width * height * 3 * SCREENRECORD_FRAMES,
        )?;
        let pixels = newest_raw_frame(&recorded.stdout, width, height)
            .ok_or_else(|| screenrecord_failure(name, &size, &recorded))?;
        let picture = dsa_host_linux::shell::png::Rgba {
            width: width as u32,
            height: height as u32,
            pixels,
        };
        let _ = std::fs::write(
            dir.join(format!("{name}-record.png")),
            dsa_host_linux::shell::png::encode(&picture, false),
        );
        Ok(picture.pixels)
    }
}

/// Why the screenrecord fallback gave no picture of `name`, which is `size` large.
pub fn screenrecord_failure(name: &str, size: &str, recorded: &Bounded) -> String {
    let how = match recorded.status {
        _ if recorded.timed_out => format!(
            "did not finish within {} s and was stopped",
            SCREENRECORD_LIMIT.as_secs()
        ),
        Some(status) => format!("ended ({status})"),
        None => "was stopped".to_string(),
    };
    format!(
        "screencap gave an empty picture of {name}, and screenrecord {how} after sending {} bytes in {:.1} s, not one whole {size} frame{}",
        recorded.stdout.len(),
        recorded.took.as_secs_f64(),
        match String::from_utf8_lossy(&recorded.stderr).trim() {
            "" => String::new(),
            said => format!("; it said: {said}"),
        }
    )
}
