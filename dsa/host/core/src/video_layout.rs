//! Where a decoded picture lies in a hardware decoder's surface, and how much of it is the
//! picture.
//!
//! A hardware decoder writes each picture into a surface larger than the picture: the coded
//! size is rounded up to the codec's block size (16 for H.264, 8 or 64 for VP9 and AV1), and
//! the surface itself is often taller still, because the driver allocates its surfaces for the
//! largest picture it expects or rounds them up again. The luma plane comes first, then the
//! chroma plane, which starts after the surface's own rows, not after the picture's.
//!
//! Getting either wrong is visible at once. Chroma read from too early a row is luma, and
//! from too late a row the zero padding below the picture: a picture whose chroma is zero is
//! solid green. A visible size taken from the coded size shows the padding as a band along the
//! right or bottom edge, green again where the padding is zero.
//!
//! Nothing here names a host operating system API, so every host's decoder uses the same
//! rules and every rule is tested on every platform.

use crate::video::{Codec, Picture};

/// A width and a height.
pub type Size = (u32, u32);

/// How a surface stores its samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceFormat {
    /// 8-bit 4:2:0, luma then interleaved Cb and Cr: one byte a sample.
    Nv12,
    /// 10-bit 4:2:0 in the layout of NV12 with two bytes a sample, the value in the top 10
    /// bits. Sent on as 8-bit NV12.
    P010,
}

impl SurfaceFormat {
    fn bytes_per_sample(self) -> usize {
        match self {
            SurfaceFormat::Nv12 => 1,
            SurfaceFormat::P010 => 2,
        }
    }
}

/// The row of the surface where the chroma plane starts, counted in luma rows.
///
/// `surface_rows` is the height of the surface the decoder wrote, when the platform can say
/// (a texture's own height); `coded_rows` is the height of the decoder's output type, and
/// `visible_rows` the picture's. The surface's height wins: its chroma plane follows its own
/// rows. Without it, the coded height, which is never less than the picture's.
pub fn chroma_start_row(
    surface_rows: Option<usize>,
    coded_rows: usize,
    visible_rows: usize,
) -> usize {
    match surface_rows {
        Some(rows) if rows >= visible_rows && rows > 0 => rows,
        _ => coded_rows.max(visible_rows),
    }
}

/// The surface height a buffer of `len` bytes with rows `pitch` bytes long holds, for a
/// 4:2:0 picture: luma rows plus half as many chroma rows. `None` when it does not divide.
pub fn rows_from_length(len: usize, pitch: usize) -> Option<usize> {
    if pitch == 0 || len % pitch != 0 {
        return None;
    }
    let total = len / pitch;
    // rows + ceil(rows / 2) == total
    let rows = (total * 2) / 3;
    (rows + rows.div_ceil(2) == total).then_some(rows)
}

/// The visible size of a picture whose decoder output type says `coded`.
///
/// `aperture` is the display area the decoder reports (H.264's and HEVC's cropping, as
/// Media Foundation's minimum display aperture carries it); `stream` the size the bitstream's
/// own header gives (VP9's frame size, AV1's sequence header), for decoders that report no
/// aperture. A size larger than the coded size is not believed: the picture cannot be larger
/// than the surface it is in. Without either, the coded size.
pub fn visible_size(coded: Size, aperture: Option<Size>, stream: Option<Size>) -> Size {
    let fits = |size: Size| size.0 > 0 && size.1 > 0 && size.0 <= coded.0 && size.1 <= coded.1;
    if let Some(size) = aperture.filter(|s| fits(*s)) {
        return size;
    }
    if let Some(size) = stream.filter(|s| fits(*s)) {
        return size;
    }
    coded
}

/// Copies the visible `width` by `height` of a picture out of a decoder's surface: rows
/// `pitch` bytes apart, the chroma plane starting `chroma_row` rows after the luma plane (see
/// [`chroma_start_row`]). P010 samples become 8-bit. The picture is NV12 with a stride of the
/// width rounded up to 2, so each chroma row holds whole pairs. `None` when `data` is too
/// short for that or the geometry does not fit.
pub fn picture_from_surface(
    id: u64,
    data: &[u8],
    format: SurfaceFormat,
    pitch: usize,
    chroma_row: usize,
    width: u32,
    height: u32,
) -> Option<Picture> {
    let (w, h) = (width as usize, height as usize);
    let sample = format.bytes_per_sample();
    let stride = w + (w & 1);
    if w == 0 || h == 0 || chroma_row < h || pitch < stride.checked_mul(sample)? {
        return None;
    }
    let chroma_rows = h.div_ceil(2);
    let needed = pitch.checked_mul(chroma_row.checked_add(chroma_rows)?)?;
    if data.len() < needed {
        return None;
    }
    let mut nv12 = Vec::with_capacity(stride * (h + chroma_rows));
    let mut copy_row = |start: usize| match format {
        SurfaceFormat::Nv12 => nv12.extend_from_slice(&data[start..start + stride]),
        // Little-endian 16-bit samples: the high byte holds the top 8 bits.
        SurfaceFormat::P010 => {
            nv12.extend((0..stride).map(|x| data[start + 2 * x + 1]));
        }
    };
    for row in 0..h {
        copy_row(row * pitch);
    }
    let chroma = chroma_row * pitch;
    for row in 0..chroma_rows {
        copy_row(chroma + row * pitch);
    }
    Some(Picture {
        id,
        width,
        height,
        stride: stride as u32,
        nv12,
    })
}

/// The picture size a packet's own header gives, for the formats whose decoders may report
/// no display area: VP9 (a key frame's or intra-only frame's size) and AV1 (the sequence
/// header's largest frame size). `None` for a packet that carries none, and always for H.264
/// and HEVC, whose decoders report their cropping.
pub fn stream_frame_size(codec: Codec, data: &[u8]) -> Option<Size> {
    match codec {
        Codec::Vp9 => vp9_frame_size(data),
        Codec::Av1 => av1_frame_size(data),
        Codec::H264 | Codec::Hevc => None,
    }
}

/// Reads bits from the front, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Bits<'a> {
        Bits { data, at: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(u32::from(bit))
    }

    fn bits(&mut self, count: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.bit()?;
        }
        Some(value)
    }

    /// AV1's uvlc().
    fn uvlc(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros >= 32 {
                return Some(u32::MAX);
            }
        }
        Some(self.bits(zeros)? + ((1u32 << zeros) - 1))
    }
}

/// The first frame of a VP9 packet, which may be a superframe (several frames with an index
/// at the end).
fn vp9_first_frame(data: &[u8]) -> &[u8] {
    let Some(&last) = data.last() else {
        return data;
    };
    if last & 0xe0 != 0xc0 {
        return data;
    }
    let frames = usize::from(last & 0x7) + 1;
    let size_bytes = usize::from((last >> 3) & 0x3) + 1;
    let index = 2 + size_bytes * frames;
    if data.len() < index || data[data.len() - index] != last {
        return data;
    }
    let start = data.len() - index + 1;
    let mut first = 0usize;
    for i in 0..size_bytes {
        first |= usize::from(data[start + i]) << (8 * i);
    }
    if first == 0 || first > data.len() - index {
        return data;
    }
    &data[..first]
}

/// A VP9 frame's size, from its uncompressed header, for a key frame or an intra-only frame.
pub fn vp9_frame_size(data: &[u8]) -> Option<Size> {
    let mut bits = Bits::new(vp9_first_frame(data));
    if bits.bits(2)? != 2 {
        return None;
    }
    let low = bits.bit()?;
    let high = bits.bit()?;
    let profile = (high << 1) | low;
    if profile == 3 {
        bits.bit()?;
    }
    if bits.bit()? == 1 {
        // show_existing_frame: a frame shown again, with no header of its own.
        return None;
    }
    let key = bits.bit()? == 0;
    let show_frame = bits.bit()?;
    let error_resilient = bits.bit()?;
    let color_config = |bits: &mut Bits| -> Option<()> {
        if profile >= 2 {
            bits.bit()?;
        }
        let color_space = bits.bits(3)?;
        if color_space != 7 {
            bits.bit()?;
            if profile == 1 || profile == 3 {
                bits.bits(3)?;
            }
        } else if profile == 1 || profile == 3 {
            bits.bit()?;
        }
        Some(())
    };
    if key {
        if bits.bits(24)? != 0x49_83_42 {
            return None;
        }
        color_config(&mut bits)?;
    } else {
        let intra_only = if show_frame == 1 { 0 } else { bits.bit()? };
        if intra_only == 0 {
            // An inter frame takes its size from a reference.
            return None;
        }
        if error_resilient == 0 {
            bits.bits(2)?;
        }
        if bits.bits(24)? != 0x49_83_42 {
            return None;
        }
        if profile > 0 {
            color_config(&mut bits)?;
        }
        bits.bits(8)?;
    }
    let width = bits.bits(16)? + 1;
    let height = bits.bits(16)? + 1;
    Some((width, height))
}

/// Reads AV1's leb128() from `data` at `at`, returning the value and the bytes it took.
fn leb128(data: &[u8], at: usize) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for i in 0..8 {
        let byte = *data.get(at + i)?;
        value |= usize::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// The largest frame size an AV1 packet's sequence header gives. The packet is a temporal
/// unit of OBUs with their sizes, or the codec configuration record (`av1C`) Android passes
/// as codec configuration, whose OBUs follow its first four bytes.
pub fn av1_frame_size(data: &[u8]) -> Option<Size> {
    let mut at = if data.first() == Some(&0x81) && data.len() > 4 {
        4
    } else {
        0
    };
    while at < data.len() {
        let header = data[at];
        let kind = (header >> 3) & 0xf;
        let extension = header & 0x4 != 0;
        let has_size = header & 0x2 != 0;
        let mut start = at + 1 + usize::from(extension);
        let size = if has_size {
            let (size, taken) = leb128(data, start)?;
            start += taken;
            size
        } else {
            data.len().checked_sub(start)?
        };
        let end = start.checked_add(size)?;
        if end > data.len() {
            return None;
        }
        // OBU_SEQUENCE_HEADER
        if kind == 1 {
            return av1_sequence_size(&data[start..end]);
        }
        at = end;
    }
    None
}

fn av1_sequence_size(header: &[u8]) -> Option<Size> {
    let mut bits = Bits::new(header);
    bits.bits(3)?; // seq_profile
    bits.bit()?; // still_picture
    let reduced = bits.bit()? == 1;
    if reduced {
        bits.bits(5)?;
    } else {
        let mut decoder_model = false;
        let mut buffer_delay_length = 0;
        if bits.bit()? == 1 {
            // timing_info
            bits.bits(32)?;
            bits.bits(32)?;
            if bits.bit()? == 1 {
                bits.uvlc()?;
            }
            decoder_model = bits.bit()? == 1;
            if decoder_model {
                buffer_delay_length = bits.bits(5)? + 1;
                bits.bits(32)?;
                bits.bits(5)?;
                bits.bits(5)?;
            }
        }
        let initial_display_delay = bits.bit()? == 1;
        let points = bits.bits(5)? + 1;
        for _ in 0..points {
            bits.bits(12)?;
            let level = bits.bits(5)?;
            if level > 7 {
                bits.bit()?;
            }
            if decoder_model && bits.bit()? == 1 {
                bits.bits(buffer_delay_length)?;
                bits.bits(buffer_delay_length)?;
                bits.bit()?;
            }
            if initial_display_delay && bits.bit()? == 1 {
                bits.bits(4)?;
            }
        }
    }
    let width_bits = bits.bits(4)? + 1;
    let height_bits = bits.bits(4)? + 1;
    let width = bits.bits(width_bits)? + 1;
    let height = bits.bits(height_bits)? + 1;
    Some((width, height))
}
