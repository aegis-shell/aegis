//! Sealed-memfd transport for large immutable IPC payloads.
//!
//! JSON framing remains small and self-describing. A capture response is
//! followed by one marker byte carrying an `SCM_RIGHTS` file descriptor. The
//! descriptor refers to a fully sealed memfd, so the receiver observes an
//! immutable byte sequence without base64 inflation or a giant JSON frame.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;

const BLOB_MARKER: u8 = 0xfd;
/// Covers the Interaction Domain model's 256-MiB maximum RGBA frame plus conservative PNG
/// container/filter overhead while still bounding every receiver allocation.
pub(crate) const MAX_BLOB_BYTES: u64 = 288 * 1024 * 1024;
const REQUIRED_SEALS: libc::c_int =
    libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;

pub(crate) struct SealedBlob {
    file: File,
    len: u64,
}

impl SealedBlob {
    pub(crate) fn new(bytes: &[u8]) -> io::Result<Self> {
        let len = u64::try_from(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "blob length overflow"))?;
        validate_len(len)?;

        // SAFETY: the name is a static NUL-terminated C string and the flags
        // are the Linux memfd API's documented bitset.
        let fd = unsafe {
            libc::memfd_create(
                c"aegis-ipc-capture".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `memfd_create` returned a new owned descriptor.
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes)?;
        file.seek(SeekFrom::Start(0))?;
        // SAFETY: `file` owns a valid memfd created with sealing enabled.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, REQUIRED_SEALS) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file, len })
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    pub(crate) fn send(&self, stream: &UnixStream) -> io::Result<()> {
        send_fd(stream, self.file.as_raw_fd())
    }
}

pub(crate) fn receive(stream: &UnixStream, expected_len: u64) -> io::Result<Vec<u8>> {
    validate_len(expected_len)?;
    let fd = receive_fd(stream)?;
    // SAFETY: `receive_fd` returns a newly received descriptor owned by the
    // caller and marked close-on-exec.
    let mut file = unsafe { File::from_raw_fd(fd) };

    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.len() != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "capture memfd length/type mismatch (expected {expected_len}, got {})",
                metadata.len()
            ),
        ));
    }
    // A malicious or buggy server must not be able to mutate pixels after
    // metadata validation or while the receiver reads them.
    // SAFETY: `file` owns a valid descriptor; F_GET_SEALS has no pointer arg.
    let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
    if seals < 0 || seals & REQUIRED_SEALS != REQUIRED_SEALS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "capture descriptor is not fully sealed",
        ));
    }

    file.seek(SeekFrom::Start(0))?;
    let len = usize::try_from(expected_len)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "capture is too large"))?;
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn validate_len(len: u64) -> io::Result<()> {
    if len == 0 || len > MAX_BLOB_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("blob length {len} is outside 1..={MAX_BLOB_BYTES}"),
        ));
    }
    Ok(())
}

/// Send one `0xfd` marker byte carrying `fd` as an `SCM_RIGHTS` descriptor.
/// Used for sealed capture memfds and for the dmabuf slot descriptors of a
/// zero-copy stream (protocol 25); the sender retains its own reference.
pub(crate) fn send_fd(stream: &UnixStream, fd: RawFd) -> io::Result<()> {
    let mut marker = BLOB_MARKER;
    let mut iov = libc::iovec {
        iov_base: (&mut marker as *mut u8).cast(),
        iov_len: 1,
    };
    // SAFETY: CMSG_SPACE computes the required control-buffer size for one
    // descriptor. The buffer remains live for the whole sendmsg call.
    let control_len = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as libc::c_uint) } as usize;
    let mut control = vec![0u8; control_len];
    // SAFETY: every pointer in the message references the live stack/vector
    // storage above and the lengths match those allocations.
    let sent = unsafe {
        let mut message: libc::msghdr = zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control.len();
        let header = libc::CMSG_FIRSTHDR(&message);
        if header.is_null() {
            return Err(io::Error::other("failed to construct SCM_RIGHTS header"));
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as libc::c_uint) as usize;
        std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<RawFd>(), fd);
        libc::sendmsg(stream.as_raw_fd(), &message, libc::MSG_NOSIGNAL)
    };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    if sent != 1 {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "capture descriptor marker was not written atomically",
        ));
    }
    Ok(())
}

/// Receive one `0xfd`-marked `SCM_RIGHTS` descriptor (the receive half of
/// [`send_fd`]). The returned descriptor is caller-owned and close-on-exec.
pub(crate) fn receive_fd(stream: &UnixStream) -> io::Result<RawFd> {
    let mut marker = 0u8;
    let mut iov = libc::iovec {
        iov_base: (&mut marker as *mut u8).cast(),
        iov_len: 1,
    };
    // SAFETY: see `send_fd`; this buffer is sized for exactly one descriptor.
    let control_len = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as libc::c_uint) } as usize;
    let mut control = vec![0u8; control_len];
    // SAFETY: all msghdr pointers target live writable storage for recvmsg.
    let (received, flags, fd) = unsafe {
        let mut message: libc::msghdr = zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control.len();
        let received = libc::recvmsg(stream.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC);
        if received < 0 {
            return Err(io::Error::last_os_error());
        }
        let header = libc::CMSG_FIRSTHDR(&message);
        let fd = if header.is_null()
            || (*header).cmsg_level != libc::SOL_SOCKET
            || (*header).cmsg_type != libc::SCM_RIGHTS
            || (*header).cmsg_len < libc::CMSG_LEN(size_of::<RawFd>() as libc::c_uint) as usize
        {
            -1
        } else {
            std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<RawFd>())
        };
        (received, message.msg_flags, fd)
    };
    if received != 1 || marker != BLOB_MARKER {
        if fd >= 0 {
            // SAFETY: the descriptor was received into this process but is
            // rejected before ownership can be wrapped by `File`.
            unsafe { libc::close(fd) };
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing capture descriptor marker",
        ));
    }
    if flags & libc::MSG_CTRUNC != 0 || fd < 0 {
        if fd >= 0 {
            // SAFETY: see the rejected-descriptor path above.
            unsafe { libc::close(fd) };
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing or truncated capture descriptor",
        ));
    }
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_blob_round_trips_over_a_unix_socket() {
        let (sender, receiver) = UnixStream::pair().unwrap();
        let blob = SealedBlob::new(b"immutable pixels").unwrap();
        blob.send(&sender).unwrap();
        assert_eq!(receive(&receiver, blob.len()).unwrap(), b"immutable pixels");
    }

    #[test]
    fn blob_length_is_bounded() {
        assert!(validate_len(0).is_err());
        assert!(validate_len(MAX_BLOB_BYTES).is_ok());
        assert!(validate_len(MAX_BLOB_BYTES + 1).is_err());
    }
}
