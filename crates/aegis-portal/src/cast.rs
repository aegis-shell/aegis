//! One PipeWire producer stream per started ScreenCast session.
//!
//! Each cast runs on its own thread: a scoped IPC connection receives the
//! compositor's output-frame stream ([ADR-0052]) and a PipeWire `Output`
//! stream republishes every frame as raw `BGRx` video. The PipeWire main
//! loop is also the IPC event loop — the IPC socket, the stop socket, and
//! the lease-renewal timer are ordinary loop sources, so the thread never
//! blocks anywhere but in `poll`.
//!
//! Teardown is single-path: closing the write end of the stop socket (or a
//! compositor-side `StreamEnded`, or any read error) quits the loop, after
//! which dropping the IPC client disconnects it — and the compositor's
//! disconnect cleanup stops the stream with no extra round-trip.
//!
//! [ADR-0052]: ../../docs/adr/0052-scoped-output-frame-streaming.md

use std::cell::RefCell;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use aegis_ipc::{Capabilities, Client, LOCAL_PORTAL_SCOPE, StreamMessage};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::pod::{self, Pod};
use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Direction, Fraction, Rectangle};
use pw::stream::{StreamFlags, StreamState};

use crate::screencast::CastJob;

/// Frame rate requested from the compositor and offered to PipeWire.
const STREAM_FPS: u32 = 30;
/// A screencast publishes frames for PipeWire capture consumers.
const STREAM_DIRECTION: Direction = Direction::Output;
/// Lease TTL requested at handshake and renewal; renewed at half TTL.
const LEASE_TTL_MS: u64 = 900_000;

/// Negotiated parameters of a running cast, handed back to the worker once
/// the stream reaches `Paused` (the first state where the node id exists).
pub(crate) struct CastStarted {
    pub(crate) node_id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Live cast handle owned by the session registry: drop `stop` to end the
/// cast, then `join` the thread before reporting the session closed.
pub(crate) struct CastHandle {
    pub(crate) stop: UnixStream,
    pub(crate) started: mpsc::Receiver<Result<CastStarted, String>>,
    pub(crate) thread: std::thread::JoinHandle<()>,
}

/// Spawn the cast thread. Returns immediately; the PipeWire negotiation
/// result arrives on `handle.started` exactly once. `window` selects the
/// window-source variant (ADR-0054): the compositor crops that window's
/// visible region from the output frame and ends the stream when the window
/// closes or its size changes.
pub(crate) fn spawn(
    socket: PathBuf,
    session_path: String,
    jobs: mpsc::Sender<CastJob>,
    window: Option<aegis_core::window::WindowId>,
) -> io::Result<CastHandle> {
    let (stop_read, stop_write) = UnixStream::pair()?;
    let (started_tx, started_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("aegis-portal-cast".to_string())
        .spawn(move || cast_thread(socket, session_path, jobs, window, stop_read, started_tx))?;
    Ok(CastHandle {
        stop: stop_write,
        started: started_rx,
        thread,
    })
}

/// `AsRawFd` wrapper so the shared IPC client can be a PipeWire loop source.
/// The timer and the IO callback both reach the client through the `Rc`;
/// PipeWire's `Fn` callbacks require interior mutability.
struct IpcFd(Rc<RefCell<Client>>);

impl AsRawFd for IpcFd {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.borrow().as_raw_fd()
    }
}

/// Latest frame shared between the IPC source (writer) and the PipeWire
/// `process` callback (reader). `None` until the first frame arrives.
type LatestFrame = Rc<RefCell<Option<Rc<Vec<u8>>>>>;

/// Stream-listener user data.
struct StreamData {
    latest: LatestFrame,
    width: u32,
    height: u32,
    started: Option<mpsc::Sender<Result<CastStarted, String>>>,
}

fn cast_thread(
    socket: PathBuf,
    session_path: String,
    jobs: mpsc::Sender<CastJob>,
    window: Option<aegis_core::window::WindowId>,
    stop_read: UnixStream,
    started: mpsc::Sender<Result<CastStarted, String>>,
) {
    if let Err(error) = run_cast(&socket, &session_path, &jobs, window, stop_read, &started) {
        log::warn!("portal: cast for {session_path} failed: {error}");
        let _ = started.send(Err(error));
    }
    // Dropping here closes the IPC connection; the compositor's disconnect
    // cleanup stops the output stream (ADR-0052).
}

/// The whole cast, as a fallible sequence. `started` is consumed by the
/// stream listener on success, so on failure the caller still owns it.
fn run_cast(
    socket: &std::path::Path,
    session_path: &str,
    jobs: &mpsc::Sender<CastJob>,
    window: Option<aegis_core::window::WindowId>,
    stop_read: UnixStream,
    started: &mpsc::Sender<Result<CastStarted, String>>,
) -> Result<(), String> {
    // Inward half first: without frames there is nothing to publish.
    let caps = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    let mut client = Client::connect_scoped(socket, caps, LOCAL_PORTAL_SCOPE)
        .map_err(|e| format!("compositor IPC connect: {e}"))?;
    let target = match window {
        Some(window) => aegis_ipc::StreamTarget::Window { window },
        None => aegis_ipc::StreamTarget::Output,
    };
    let stream_info = client
        .start_output_stream_target(Some(STREAM_FPS), target)
        .map_err(|e| format!("start output stream: {e}"))?;
    let (width, height) = (stream_info.width, stream_info.height);
    log::info!(
        "portal: compositor stream {} for {session_path}: {width}x{height}@{STREAM_FPS}",
        stream_info.stream_id
    );
    let client = Rc::new(RefCell::new(client));

    // Outward half: PipeWire producer.
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| e.to_string())?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "aegis-portal-screencast",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|e| e.to_string())?;

    let latest: LatestFrame = Rc::new(RefCell::new(None));
    let format_pod = format_pod(width, height);

    let _listener = stream
        .add_local_listener_with_user_data(StreamData {
            latest: Rc::clone(&latest),
            width,
            height,
            started: Some(started.clone()),
        })
        .state_changed(|stream, data, _old, new| {
            log::debug!("portal: pipewire stream {new:?}");
            // Paused is the first state with a valid node id; report the
            // started parameters exactly once.
            if new == StreamState::Paused
                && let Some(started) = data.started.take()
            {
                let _ = started.send(Ok(CastStarted {
                    node_id: stream.node_id(),
                    width: data.width,
                    height: data.height,
                }));
            }
        })
        .param_changed(|stream, data, id, param| {
            if id == spa::param::ParamType::Format.as_raw() && param.is_some() {
                let buffers = buffers_pod(data.width, data.height);
                let mut params = [Pod::from_bytes(&buffers).expect("buffers pod")];
                if let Err(error) = stream.update_params(&mut params) {
                    log::warn!("portal: pipewire update_params failed: {error}");
                }
            }
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let data_ref = &mut datas[0];
            let stride = data.width as usize * 4;
            let frame = data.latest.borrow();
            // Copy the latest compositor frame into the buffer. A missing or
            // oversized frame yields a zero-sized chunk, which PipeWire
            // treats as "no new content" instead of corrupt pixels.
            let size = match frame.as_ref() {
                Some(frame) => match data_ref.data() {
                    Some(dest) if dest.len() >= frame.len() => {
                        dest[..frame.len()].copy_from_slice(frame);
                        frame.len()
                    }
                    _ => 0,
                },
                None => 0,
            };
            let chunk = data_ref.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.size_mut() = size as u32;
            *chunk.stride_mut() = stride as i32;
        })
        .register()
        .map_err(|e| e.to_string())?;

    // IPC frames arrive as a loop source; the lease timer keeps the scoped
    // connection alive across quiet periods.
    let loop_weak = mainloop.downgrade();
    let _ipc_source = mainloop.loop_().add_io(
        IpcFd(Rc::clone(&client)),
        spa::support::system::IoFlags::IN | spa::support::system::IoFlags::ERR,
        {
            let jobs = jobs.clone();
            let session_path = session_path.to_string();
            move |io| {
                let message = io.0.borrow_mut().next_stream_message();
                match message {
                    Ok(StreamMessage::Frame(frame)) => match frame.format {
                        aegis_ipc::StreamPixelFormat::Bgra8
                        | aegis_ipc::StreamPixelFormat::Rgba8 => {
                            if frame.pixels.len() == (width as usize) * (height as usize) * 4 {
                                *latest.borrow_mut() = Some(Rc::new(frame.pixels));
                            }
                        }
                        aegis_ipc::StreamPixelFormat::Dmabuf { .. } => {
                            if frame.pixels.len() == (width as usize) * (height as usize) * 4 {
                                *latest.borrow_mut() = Some(Rc::new(frame.pixels));
                            }
                        }
                    },
                    Ok(StreamMessage::LeaseRenewed) => {}
                    Ok(StreamMessage::Ended { reason, .. }) => {
                        log::info!("portal: compositor ended stream for {session_path}: {reason}");
                        end_cast(&jobs, &session_path, &loop_weak);
                    }
                    Err(error) => {
                        log::warn!("portal: stream read for {session_path} failed: {error}");
                        end_cast(&jobs, &session_path, &loop_weak);
                    }
                }
            }
        },
    );

    let _stop_source = mainloop.loop_().add_io(
        stop_read,
        spa::support::system::IoFlags::IN | spa::support::system::IoFlags::HUP,
        {
            let loop_weak = mainloop.downgrade();
            move |socket| {
                let mut byte = [0u8; 1];
                use std::io::Read;
                // EOF (write end dropped) is the only expected readability.
                let _ = socket.read(&mut byte);
                if let Some(mainloop) = loop_weak.upgrade() {
                    mainloop.quit();
                }
            }
        },
    );

    let lease_timer = mainloop.loop_().add_timer(move |_| {
        if let Err(error) = client.borrow_mut().request_lease_renewal(LEASE_TTL_MS) {
            log::warn!("portal: cast lease renewal failed: {error}");
        }
    });
    let half_ttl = Duration::from_millis(LEASE_TTL_MS / 2);
    lease_timer.update_timer(Some(half_ttl), Some(half_ttl));

    let mut params = [Pod::from_bytes(&format_pod).expect("format pod")];
    stream
        .connect(
            // This stream publishes compositor frames. `Input` describes a
            // capture consumer (for example a camera reader) and leaves OBS
            // with no producer port to link to; a screencast source must
            // expose an output port.
            STREAM_DIRECTION,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    mainloop.run();
    Ok(())
}

/// Report a compositor-side stream end to the screencast worker and stop
/// the cast thread's loop.
fn end_cast(
    jobs: &mpsc::Sender<CastJob>,
    session_path: &str,
    loop_weak: &pw::main_loop::MainLoopWeak,
) {
    let _ = jobs.send(CastJob::SessionEnded {
        session_path: session_path.to_string(),
    });
    if let Some(mainloop) = loop_weak.upgrade() {
        mainloop.quit();
    }
}

/// Offered video format: raw BGRx at the output's geometry, variable
/// framerate. The compositor's stream format is BGRA with opaque alpha,
/// which is exactly `BGRx` semantics (ADR-0052).
fn format_pod(width: u32, height: u32) -> Vec<u8> {
    let object = pod::object! {
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Id,
            spa::param::video::VideoFormat::BGRx
        ),
        pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Rectangle,
            Rectangle { width, height }
        ),
        pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Fraction,
            Fraction { num: 0, denom: 1 }
        ),
    };
    serialize(&pod::Value::Object(object))
}

/// Buffer constraints offered once the format is negotiated: 2–8 shared or
/// plain buffers of exactly one frame. MemPtr | MemFd = 1 | 2 = 3 (masks of
/// `enum spa_data_type`).
fn buffers_pod(width: u32, height: u32) -> Vec<u8> {
    let stride = width as i32 * 4;
    let size = stride * height as i32;
    let object = pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            pod::Property {
                key: 1, // SPA_PARAM_BUFFERS_buffers
                flags: pod::PropertyFlags::empty(),
                value: pod::Value::Choice(pod::ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: 4,
                        min: 2,
                        max: 8,
                    },
                ))),
            },
            pod::Property {
                key: 2, // SPA_PARAM_BUFFERS_blocks
                flags: pod::PropertyFlags::empty(),
                value: pod::Value::Int(1),
            },
            pod::Property {
                key: 3, // SPA_PARAM_BUFFERS_size
                flags: pod::PropertyFlags::empty(),
                value: pod::Value::Int(size),
            },
            pod::Property {
                key: 4, // SPA_PARAM_BUFFERS_stride
                flags: pod::PropertyFlags::empty(),
                value: pod::Value::Int(stride),
            },
            pod::Property {
                key: 6, // SPA_PARAM_BUFFERS_dataType
                flags: pod::PropertyFlags::empty(),
                value: pod::Value::Choice(pod::ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: 1,
                        flags: vec![1, 2],
                    },
                ))),
            },
        ],
    };
    serialize(&pod::Value::Object(object))
}

fn serialize(value: &pod::Value) -> Vec<u8> {
    pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), value)
        .expect("pod serialization")
        .0
        .into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod_words(bytes: &[u8]) -> &[u32] {
        let (head, words, tail) = unsafe { bytes.align_to::<u32>() };
        assert!(head.is_empty() && tail.is_empty());
        words
    }

    #[test]
    fn screencast_stream_is_a_pipewire_producer() {
        assert!(matches!(STREAM_DIRECTION, Direction::Output));
    }

    #[test]
    fn format_pod_is_a_parseable_video_format() {
        let bytes = format_pod(1920, 1080);
        assert!(Pod::from_bytes(&bytes).is_some(), "pod parses");
        let words = pod_words(&bytes);
        // Pod header: body size, then SPA_TYPE_Object; the object body
        // carries type (ObjectParamFormat) and id (EnumFormat).
        assert_eq!(words[0] as usize, bytes.len() - 8);
        assert_eq!(words[2], spa::utils::SpaTypes::ObjectParamFormat.as_raw());
        assert_eq!(words[3], spa::param::ParamType::EnumFormat.as_raw());
        // Round-trip through the deserializer to prove the shape is valid.
        let parsed = pod::deserialize::PodDeserializer::deserialize_from::<pod::Value>(&bytes)
            .expect("deserialize")
            .1;
        let pod::Value::Object(object) = parsed else {
            panic!("expected an object pod");
        };
        assert_eq!(object.properties.len(), 5);
    }

    #[test]
    fn buffers_pod_covers_one_frame() {
        let bytes = buffers_pod(640, 480);
        let parsed = pod::deserialize::PodDeserializer::deserialize_from::<pod::Value>(&bytes)
            .expect("deserialize")
            .1;
        let pod::Value::Object(object) = parsed else {
            panic!("expected an object pod");
        };
        assert_eq!(
            object.type_,
            spa::utils::SpaTypes::ObjectParamBuffers.as_raw()
        );
        let size = object
            .properties
            .iter()
            .find(|p| p.key == 3)
            .and_then(|p| match p.value {
                pod::Value::Int(size) => Some(size),
                _ => None,
            })
            .expect("size property");
        assert_eq!(size, 640 * 480 * 4);
    }
}
