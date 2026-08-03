use crate::*;

/// Maximum aggregate size retained by one compositor-owned clipboard
/// selection. This matches the IPC capture blob ceiling and prevents an
/// internal caller from pinning unbounded memory until the next selection.
const MAX_OWNED_CLIPBOARD_BYTES: usize = 288 * 1024 * 1024;
const MAX_OWNED_CLIPBOARD_MIME_TYPES: usize = 32;
const MAX_CLIPBOARD_MIME_LEN: usize = 255;
const CLIPBOARD_WRITE_QUEUE_DEPTH: usize = 8;

/// Errors installing compositor-owned clipboard data.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClipboardError {
    #[error("seat {} is unknown, paused, or revoked", .0.0)]
    SeatUnavailable(SeatId),
    #[error("clipboard data must contain at least one MIME payload")]
    Empty,
    #[error("clipboard data contains too many MIME payloads")]
    TooManyMimeTypes,
    #[error("invalid clipboard MIME type {0:?}")]
    InvalidMime(String),
    #[error("duplicate clipboard MIME type {0:?}")]
    DuplicateMime(String),
    #[error("clipboard data exceeds the {MAX_OWNED_CLIPBOARD_BYTES}-byte limit")]
    TooLarge,
}

impl Server {
    /// Replace one seat's clipboard with immutable compositor-owned data.
    ///
    /// The selection is visible only to the focused client on `seat`; it is
    /// never copied into another Realm. Payload transfer happens off the
    /// compositor thread and remains valid after the caller drops its bytes.
    pub fn set_clipboard_data(
        &mut self,
        seat: SeatId,
        payloads: Vec<(String, Vec<u8>)>,
    ) -> Result<(), ClipboardError> {
        install_owned_clipboard(self.state.as_mut(), seat, payloads)
    }

    /// Arc-preserving variant for large compositor-produced payloads such as
    /// screenshots. The capture worker and the clipboard selection can share
    /// one immutable PNG allocation while the worker finishes the independent
    /// atomic disk write.
    pub fn set_clipboard_data_shared(
        &mut self,
        seat: SeatId,
        payloads: Vec<(String, std::sync::Arc<[u8]>)>,
    ) -> Result<(), ClipboardError> {
        install_owned_clipboard_shared(self.state.as_mut(), seat, payloads)
    }
}

fn install_owned_clipboard(
    state: &mut State,
    seat: SeatId,
    payloads: Vec<(String, Vec<u8>)>,
) -> Result<(), ClipboardError> {
    install_owned_clipboard_shared(
        state,
        seat,
        payloads
            .into_iter()
            .map(|(mime, bytes)| (mime, std::sync::Arc::from(bytes)))
            .collect(),
    )
}

fn install_owned_clipboard_shared(
    state: &mut State,
    seat: SeatId,
    payloads: Vec<(String, std::sync::Arc<[u8]>)>,
) -> Result<(), ClipboardError> {
    let selection = build_owned_selection_shared(payloads)?;
    let Some(_guard) = ActiveSeatGuard::enter(state, seat) else {
        return Err(ClipboardError::SeatUnavailable(seat));
    };
    unsafe { replace_clipboard_selection(state, Some(selection)) };
    Ok(())
}

#[cfg(test)]
fn build_owned_selection(payloads: Vec<(String, Vec<u8>)>) -> Result<Selection, ClipboardError> {
    build_owned_selection_shared(
        payloads
            .into_iter()
            .map(|(mime, bytes)| (mime, std::sync::Arc::from(bytes)))
            .collect(),
    )
}

fn build_owned_selection_shared(
    payloads: Vec<(String, std::sync::Arc<[u8]>)>,
) -> Result<Selection, ClipboardError> {
    if payloads.is_empty() {
        return Err(ClipboardError::Empty);
    }
    if payloads.len() > MAX_OWNED_CLIPBOARD_MIME_TYPES {
        return Err(ClipboardError::TooManyMimeTypes);
    }

    let mut total = 0usize;
    let mut mime_types = Vec::with_capacity(payloads.len());
    let mut owned = Vec::with_capacity(payloads.len());
    for (mime, bytes) in payloads {
        if mime.is_empty()
            || mime.len() > MAX_CLIPBOARD_MIME_LEN
            || mime.as_bytes().contains(&0)
            || !mime.contains('/')
        {
            return Err(ClipboardError::InvalidMime(mime));
        }
        if mime_types.iter().any(|existing| existing == &mime) {
            return Err(ClipboardError::DuplicateMime(mime));
        }
        total = total
            .checked_add(bytes.len())
            .ok_or(ClipboardError::TooLarge)?;
        if total > MAX_OWNED_CLIPBOARD_BYTES {
            return Err(ClipboardError::TooLarge);
        }
        mime_types.push(mime.clone());
        owned.push((mime, bytes));
    }

    Ok(Selection {
        source: std::ptr::null_mut(),
        mime_types,
        owned: Some(OwnedSelection {
            payloads: std::sync::Arc::new(owned),
        }),
    })
}

struct ClipboardWriteJob {
    fd: i32,
    bytes: std::sync::Arc<[u8]>,
}

enum ClipboardWriter {
    Ready(std::sync::mpsc::SyncSender<ClipboardWriteJob>),
    Unavailable,
}

fn clipboard_writer() -> &'static ClipboardWriter {
    static WRITER: std::sync::OnceLock<ClipboardWriter> = std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        let (tx, rx) =
            std::sync::mpsc::sync_channel::<ClipboardWriteJob>(CLIPBOARD_WRITE_QUEUE_DEPTH);
        match std::thread::Builder::new()
            .name("aegis-clipboard".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    use std::io::Write;
                    use std::os::fd::FromRawFd;

                    let mut file = unsafe { std::fs::File::from_raw_fd(job.fd) };
                    if let Err(error) = file.write_all(&job.bytes) {
                        log::debug!("clipboard transfer failed: {error}");
                    }
                }
            }) {
            Ok(_) => ClipboardWriter::Ready(tx),
            Err(error) => {
                log::error!("could not start clipboard transfer worker: {error}");
                ClipboardWriter::Unavailable
            }
        }
    })
}

/// Transfer a compositor-owned offer without blocking the Wayland dispatch
/// thread on pipe backpressure. The queue is deliberately bounded; a client
/// that stops reading cannot cause unbounded jobs or memory growth.
pub(crate) fn queue_owned_clipboard_write(fd: i32, bytes: std::sync::Arc<[u8]>) {
    if fd < 0 {
        return;
    }
    let job = ClipboardWriteJob { fd, bytes };
    let rejected = match clipboard_writer() {
        ClipboardWriter::Ready(tx) => match tx.try_send(job) {
            Ok(()) => return,
            Err(std::sync::mpsc::TrySendError::Full(job))
            | Err(std::sync::mpsc::TrySendError::Disconnected(job)) => job,
        },
        ClipboardWriter::Unavailable => job,
    };
    unsafe { libc_close(rejected.fd) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_clipboard_validates_mime_types() {
        assert_eq!(
            build_owned_selection(Vec::new()).err().unwrap(),
            ClipboardError::Empty
        );
        assert!(matches!(
            build_owned_selection(vec![("png".into(), vec![1])]),
            Err(ClipboardError::InvalidMime(_))
        ));
        assert!(matches!(
            build_owned_selection(vec![
                ("image/png".into(), vec![1]),
                ("image/png".into(), vec![2]),
            ]),
            Err(ClipboardError::DuplicateMime(_))
        ));
    }

    #[test]
    fn shared_clipboard_keeps_the_original_payload_allocation() {
        let png: std::sync::Arc<[u8]> = std::sync::Arc::from(&b"png-bytes"[..]);
        let selection =
            build_owned_selection_shared(vec![("image/png".into(), std::sync::Arc::clone(&png))])
                .unwrap();
        let installed = selection
            .owned
            .as_ref()
            .unwrap()
            .payload("image/png")
            .unwrap();
        assert!(std::sync::Arc::ptr_eq(&png, &installed));
    }

    #[test]
    fn owned_clipboard_is_scoped_to_the_requested_seat() {
        let mut state = State::new(std::ptr::null_mut());
        let agent = state
            .authority
            .create_agent_realm("clipboard-agent", SeatCapabilities::POINTER_KEYBOARD);
        state.seats.insert(
            agent.seat,
            Box::new(SeatRuntime::new(
                agent.seat,
                agent.realm,
                agent.principal,
                SeatCapabilities::POINTER_KEYBOARD,
            )),
        );

        install_owned_clipboard(
            &mut state,
            HUMAN_SEAT,
            vec![("image/png".into(), vec![1, 2, 3])],
        )
        .unwrap();
        assert!(state.seat_runtime(HUMAN_SEAT).unwrap().selection.is_some());
        assert!(state.seat_runtime(agent.seat).unwrap().selection.is_none());

        install_owned_clipboard(
            &mut state,
            agent.seat,
            vec![("text/plain".into(), b"agent".to_vec())],
        )
        .unwrap();
        let human = state
            .seat_runtime(HUMAN_SEAT)
            .unwrap()
            .selection
            .as_ref()
            .unwrap()
            .owned
            .as_ref()
            .unwrap();
        let agent_owned = state
            .seat_runtime(agent.seat)
            .unwrap()
            .selection
            .as_ref()
            .unwrap()
            .owned
            .as_ref()
            .unwrap();
        assert_eq!(&*human.payload("image/png").unwrap(), &[1, 2, 3]);
        assert_eq!(&*agent_owned.payload("text/plain").unwrap(), b"agent");
    }

    #[test]
    fn owned_clipboard_transfer_writes_and_closes_the_destination_fd() {
        use std::io::Read;
        use std::os::fd::IntoRawFd;

        let (reader, writer) = std::os::unix::net::UnixStream::pair().unwrap();
        queue_owned_clipboard_write(writer.into_raw_fd(), std::sync::Arc::from(&b"png"[..]));
        let mut received = Vec::new();
        (&reader).read_to_end(&mut received).unwrap();
        assert_eq!(received, b"png");
    }
}
