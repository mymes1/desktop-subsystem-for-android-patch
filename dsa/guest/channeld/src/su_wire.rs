//! Framing for the local su client's standard streams.

use std::io::{self, Read, Write};

/// The largest payload in one message. Long streams use several messages.
pub const MAX_PAYLOAD: usize = 64 * 1024;
pub const START: u8 = 1;
pub const STDIN: u8 = 2;
pub const STDIN_EOF: u8 = 3;
pub const STDOUT: u8 = 4;
pub const STDERR: u8 = 5;
pub const EXIT: u8 = 6;

/// A local message, with a one-byte type and a bounded payload.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub payload: Vec<u8>,
}

/// Read one message, rejecting a length before allocating its payload.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Frame> {
    let mut header = [0u8; 5];
    reader.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    if len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "su frame is too large",
        ));
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload)?;
    Ok(Frame {
        kind: header[0],
        payload,
    })
}

/// Write one complete message.
pub fn write_frame(writer: &mut impl Write, frame: &Frame) -> io::Result<()> {
    if frame.payload.len() > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "su frame is too large",
        ));
    }
    writer.write_all(&[frame.kind])?;
    writer.write_all(&(frame.payload.len() as u32).to_be_bytes())?;
    writer.write_all(&frame.payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr35_su_stream_messages_round_trip_without_losing_bytes() {
        let expected = Frame {
            kind: 3,
            payload: vec![0, 255, 10, 0],
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).unwrap();
        assert_eq!(read_frame(&mut bytes.as_slice()).unwrap(), expected);
    }

    #[test]
    fn fr35_a_bad_su_frame_is_rejected_without_allocating_its_payload() {
        let mut too_large = [1, 0, 1, 0, 1].as_slice();
        assert_eq!(
            read_frame(&mut too_large).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut truncated = [1, 0, 0, 0, 2, 4].as_slice();
        assert_eq!(
            read_frame(&mut truncated).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
