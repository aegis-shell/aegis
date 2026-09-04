//! Length-prefixed JSON framing for the IPC.
//!
//! Each message is `[u32 little-endian length][JSON bytes]`. The length cap
//! rejects a hostile peer that asks the reader to allocate gigabytes. The
//! payload is a `serde` value, so the wire is self-describing and new
//! variants add without changing the framing. See ADR-0027.

use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};
use zeroize::Zeroize as _;

/// Frames larger than this are rejected before allocation, bounding the
/// memory a misbehaving peer can make the reader reserve.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Serialize and write one framed message, then flush.
pub fn write_msg<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(msg).map_err(json_io_err)?;
    if bytes.len() > MAX_FRAME {
        bytes.zeroize();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {} exceeds {MAX_FRAME}", bytes.len()),
        ));
    }
    let len = bytes.len() as u32;
    let result = w
        .write_all(&len.to_le_bytes())
        .and_then(|_| w.write_all(&bytes))
        .and_then(|_| w.flush());
    bytes.zeroize();
    result
}

/// Read and deserialize one framed message. Any read error — including the
/// clean EOF a peer produces by closing between frames — is returned so the
/// caller can treat it as a disconnect.
pub fn read_msg<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds {MAX_FRAME}"),
        ));
    }
    let mut bytes = vec![0u8; len];
    r.read_exact(&mut bytes)?;
    let result = serde_json::from_slice(&bytes).map_err(json_io_err);
    bytes.zeroize();
    result
}

fn json_io_err(e: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{PROTOCOL_VERSION, Request};
    use std::io::Cursor;

    #[test]
    fn round_trip_through_a_buffer() {
        let req = Request::Hello {
            version: PROTOCOL_VERSION,
            caps: crate::schema::ConnectionCapabilities::QUERY,
            scope: None,
            lease: None,
            agent: None,
        };
        let mut buf = Vec::new();
        write_msg(&mut buf, &req).unwrap();
        let mut cur = Cursor::new(&buf);
        let back: Request = read_msg(&mut cur).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn truncated_header_is_an_error() {
        // Only 2 bytes of the length header → read_exact hits EOF.
        let mut cur = Cursor::new(&[0u8, 1][..]);
        let r: io::Result<Request> = read_msg(&mut cur);
        assert!(r.is_err());
    }

    #[test]
    fn oversize_length_is_rejected_before_alloc() {
        let mut buf = Vec::new();
        // length = 17 MiB, just over MAX_FRAME.
        buf.extend_from_slice(&((MAX_FRAME as u32) + 1).to_le_bytes());
        buf.extend_from_slice(b"{}");
        let mut cur = Cursor::new(&buf);
        let r: io::Result<Request> = read_msg(&mut cur);
        let err = r.unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{}", err);
    }

    #[test]
    fn oversize_write_is_rejected_before_emitting_a_header() {
        let mut output = Vec::new();
        let value = "x".repeat(MAX_FRAME + 1);
        let err = write_msg(&mut output, &value).unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{err}");
        assert!(output.is_empty());
    }
}
