//! `org.freedesktop.impl.portal.Session` objects and the cast-session
//! registry behind the ScreenCast portal (ADR-0051 Phase 2).
//!
//! One Session object per `CreateSession` call, registered at the
//! `session_handle_token`-derived path. `Close` hands the path to the
//! screencast worker, which stops the cast (if any), emits `Closed`, and
//! removes the object. A cast that ends from the compositor side
//! (`Event::StreamEnded`, disconnect) takes the same teardown path.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;

/// The source a session is armed with (ADR-0054). `Window` carries the
/// compositor's window id; the cast crops that window's visible region from
/// the output frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CastSource {
    Monitor,
    Window(aegis_core::window::WindowId),
}

/// State of one portal screencast session.
pub(crate) struct CastSession {
    #[allow(dead_code)]
    pub(crate) app_id: String,
    pub(crate) sources_selected: bool,
    /// The armed source; meaningful once `sources_selected` holds.
    pub(crate) source: CastSource,
    /// The restore token `Start` reports (ScreenCast v2). `Some` only for
    /// `persist_mode` 1/2 sessions.
    pub(crate) restore_token: Option<String>,
    /// Closing this end makes the cast thread's stop socket readable, which
    /// quits its PipeWire main loop. `None` until `Start` succeeds.
    pub(crate) stop: Option<UnixStream>,
    pub(crate) cast_thread: Option<std::thread::JoinHandle<()>>,
}

/// Live sessions keyed by object path.
#[derive(Default)]
pub(crate) struct SessionRegistry {
    sessions: HashMap<String, CastSession>,
}

impl SessionRegistry {
    /// Register a fresh session. Duplicate paths are refused so a hostile or
    /// buggy client cannot shadow another application's session.
    pub(crate) fn insert(&mut self, path: &str, app_id: String) -> Result<(), String> {
        if self.sessions.contains_key(path) {
            return Err(format!("session {path} already exists"));
        }
        self.sessions.insert(
            path.to_string(),
            CastSession {
                app_id,
                sources_selected: false,
                source: CastSource::Monitor,
                restore_token: None,
                stop: None,
                cast_thread: None,
            },
        );
        Ok(())
    }

    pub(crate) fn contains(&self, path: &str) -> bool {
        self.sessions.contains_key(path)
    }

    pub(crate) fn mark_sources_selected(
        &mut self,
        path: &str,
        restore_token: Option<String>,
        source: CastSource,
    ) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(path)
            .ok_or_else(|| format!("unknown session {path}"))?;
        if session.sources_selected {
            return Err(format!("session {path} already selected sources"));
        }
        session.sources_selected = true;
        session.source = source;
        session.restore_token = restore_token;
        Ok(())
    }

    /// The source `Start` must cast (ADR-0054).
    pub(crate) fn source(&self, path: &str) -> Option<CastSource> {
        self.sessions.get(path).map(|session| session.source)
    }

    /// The restore token `Start` must report, if the session was armed with
    /// one (ScreenCast v2).
    pub(crate) fn restore_token(&self, path: &str) -> Option<String> {
        self.sessions.get(path)?.restore_token.clone()
    }

    /// Whether `Start` may proceed for this session.
    pub(crate) fn can_start(&self, path: &str) -> Result<(), String> {
        let session = self
            .sessions
            .get(path)
            .ok_or_else(|| format!("unknown session {path}"))?;
        if !session.sources_selected {
            return Err(format!("session {path} has not selected sources"));
        }
        if session.cast_thread.is_some() {
            return Err(format!("session {path} already started"));
        }
        Ok(())
    }

    pub(crate) fn mark_started(
        &mut self,
        path: &str,
        stop: UnixStream,
        cast_thread: std::thread::JoinHandle<()>,
    ) {
        if let Some(session) = self.sessions.get_mut(path) {
            session.stop = Some(stop);
            session.cast_thread = Some(cast_thread);
        }
    }

    /// Remove the session and stop its cast: dropping `stop` signals the
    /// cast thread, and the thread is joined so its PipeWire stream and IPC
    /// connection are gone before the caller emits `Closed`.
    pub(crate) fn remove(&mut self, path: &str) -> Option<CastSession> {
        let mut session = self.sessions.remove(path)?;
        drop(session.stop.take());
        if let Some(thread) = session.cast_thread.take() {
            let _ = thread.join();
        }
        Some(session)
    }
}

/// The served session object. The portal spec gives it `Close` and the
/// `Closed` signal; the worker emits the signal after teardown so the
/// frontend observes a fully stopped session.
pub(crate) struct SessionIface {
    pub(crate) path: String,
    pub(crate) jobs: std::sync::mpsc::Sender<crate::screencast::CastJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Session")]
impl SessionIface {
    async fn close(&self) -> zbus::fdo::Result<()> {
        log::info!("portal: session {} closed by client", self.path);
        self.jobs
            .send(crate::screencast::CastJob::CloseSession {
                session_path: self.path.clone(),
            })
            .map_err(|_| zbus::fdo::Error::Failed("screencast worker is gone".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_with(path: &str) -> SessionRegistry {
        let mut registry = SessionRegistry::default();
        registry.insert(path, "org.example.App".into()).unwrap();
        registry
    }

    #[test]
    fn duplicate_session_paths_are_refused() {
        let mut registry = registry_with("/s/1");
        assert!(registry.contains("/s/1"));
        assert!(registry.insert("/s/1", "other".into()).is_err());
        assert!(registry.insert("/s/2", "other".into()).is_ok());
    }

    #[test]
    fn start_requires_selected_sources_and_single_use() {
        let mut registry = registry_with("/s/1");
        assert!(registry.can_start("/s/1").is_err());
        registry
            .mark_sources_selected("/s/1", None, CastSource::Monitor)
            .unwrap();
        assert!(registry.can_start("/s/1").is_ok());
        assert!(
            registry
                .mark_sources_selected("/s/1", None, CastSource::Monitor)
                .is_err()
        );

        let (stop, _read) = UnixStream::pair().unwrap();
        let thread = std::thread::spawn(|| {});
        registry.mark_started("/s/1", stop, thread);
        assert!(registry.can_start("/s/1").is_err());
    }

    #[test]
    fn remove_stops_and_joins_the_cast() {
        let mut registry = registry_with("/s/1");
        registry
            .mark_sources_selected("/s/1", None, CastSource::Monitor)
            .unwrap();
        let (stop, read) = UnixStream::pair().unwrap();
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_flag = std::sync::Arc::clone(&flag);
        let thread = std::thread::spawn(move || {
            let mut byte = [0u8; 1];
            use std::io::Read;
            let _ = (&read).read(&mut byte);
            thread_flag.store(true, std::sync::atomic::Ordering::Release);
        });
        registry.mark_started("/s/1", stop, thread);
        let session = registry.remove("/s/1").expect("session was live");
        drop(session);
        assert!(flag.load(std::sync::atomic::Ordering::Acquire));
        assert!(!registry.contains("/s/1"));
    }
}
