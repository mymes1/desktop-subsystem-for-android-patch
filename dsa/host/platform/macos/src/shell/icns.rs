//! `.icns` files from PNG images.
//!
//! An `.icns` file is a list of typed chunks after an eight-byte header, all lengths
//! big-endian. Since macOS 10.7 a chunk may hold a PNG as it is, so an Android app's launcher
//! icon becomes a bundle icon without being decoded.

/// The chunk type for a PNG `size` pixels square, the largest the PNG fits when it is not one
/// of the sizes macOS names.
pub fn chunk_type(size: u32) -> [u8; 4] {
    match size {
        1024.. => *b"ic10",
        512.. => *b"ic09",
        256.. => *b"ic08",
        128.. => *b"ic07",
        64.. => *b"icp6",
        32.. => *b"icp5",
        _ => *b"icp4",
    }
}

/// The width and height a PNG says it has, from its header chunk.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || &png[..8] != SIGNATURE || &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((width, height))
}

/// An `.icns` file holding `png`. Fails for data that is not a PNG.
pub fn from_png(png: &[u8]) -> Result<Vec<u8>, String> {
    let (width, height) = png_size(png).ok_or("the app's icon is not a PNG image")?;
    let chunk_len = u32::try_from(png.len() + 8).map_err(|_| "the app's icon is too large")?;
    let total = chunk_len
        .checked_add(8)
        .ok_or("the app's icon is too large")?;
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&chunk_type(width.min(height)));
    out.extend_from_slice(&chunk_len.to_be_bytes());
    out.extend_from_slice(png);
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PNG header claiming `size` square; enough for the parts that only read the header.
    pub fn png_header(size: u32) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&size.to_be_bytes());
        png.extend_from_slice(&size.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        png
    }

    #[test]
    fn fr7_an_apps_icon_becomes_a_bundle_icon_unchanged() {
        let png = png_header(192);
        let icns = from_png(&png).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(
            u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize,
            icns.len()
        );
        assert_eq!(&icns[8..12], b"ic07");
        assert_eq!(
            u32::from_be_bytes(icns[12..16].try_into().unwrap()) as usize,
            png.len() + 8
        );
        assert_eq!(&icns[16..], &png[..]);
    }

    #[test]
    fn fr7_each_icon_size_gets_the_chunk_macos_names_for_it() {
        assert_eq!(&chunk_type(1024), b"ic10");
        assert_eq!(&chunk_type(512), b"ic09");
        assert_eq!(&chunk_type(432), b"ic08");
        assert_eq!(&chunk_type(48), b"icp5");
        assert_eq!(&chunk_type(16), b"icp4");
    }

    #[test]
    fn fr7_data_that_is_not_a_png_is_refused() {
        assert!(from_png(b"GIF89a").is_err());
        assert_eq!(png_size(&png_header(64)), Some((64, 64)));
    }
}
