//! Session persistence: one JSONL file per conversation under
//! `$XDG_DATA_HOME/fuji/sessions/`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent::provider::Message;

/// A conversation and its append-only on-disk log.
pub struct Session {
    id: String,
    path: PathBuf,
    /// Full message history; the loop mutates it in place.
    pub messages: Vec<Message>,
}

impl Session {
    /// Start a fresh session with a timestamped id.
    pub fn create(data_dir: &Path) -> Result<Self, SessionError> {
        let dir = sessions_dir(data_dir);
        std::fs::create_dir_all(&dir).map_err(|source| SessionError::Read {
            path: dir.clone(),
            source,
        })?;
        let id = format!("{}-{}", timestamp(), std::process::id());
        Ok(Self {
            path: dir.join(format!("{id}.jsonl")),
            id,
            messages: Vec::new(),
        })
    }

    /// Load a session by id, or the newest one for `latest`.
    pub fn load(data_dir: &Path, id_or_latest: &str) -> Result<Self, SessionError> {
        let dir = sessions_dir(data_dir);
        let id = if id_or_latest == "latest" {
            Self::list(data_dir)?
                .into_iter()
                .last()
                .ok_or(SessionError::NoSessions)?
        } else {
            id_or_latest.to_string()
        };
        let path = dir.join(format!("{id}.jsonl"));
        let text = std::fs::read_to_string(&path).map_err(|source| SessionError::Read {
            path: path.clone(),
            source,
        })?;
        let mut messages = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            messages.push(serde_json::from_str(line)?);
        }
        Ok(Self { id, path, messages })
    }

    /// All session ids, oldest first.
    pub fn list(data_dir: &Path) -> Result<Vec<String>, SessionError> {
        let dir = sessions_dir(data_dir);
        let mut ids = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
            Err(source) => return Err(SessionError::Read { path: dir, source }),
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(id) = name.strip_suffix(".jsonl") {
                ids.push(id.to_string());
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Append messages from `start..` of the history to the log. The agent
    /// loop owns the in-memory history; call this after each completed turn.
    pub fn flush_from(&mut self, start: usize) -> Result<(), SessionError> {
        if start >= self.messages.len() {
            return Ok(());
        }
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| SessionError::Read {
                path: self.path.clone(),
                source,
            })?;
        for message in &self.messages[start..] {
            let mut line = serde_json::to_vec(message)?;
            line.push(b'\n');
            file.write_all(&line).map_err(|source| SessionError::Read {
                path: self.path.clone(),
                source,
            })?;
        }
        Ok(())
    }
}

fn sessions_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sessions")
}

/// `YYYYMMDD-HHMMSS` in UTC, without a calendar dependency.
pub(crate) fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Days since epoch → (year, month, day); Howard Hinnant's civil algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = (days - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("cannot access session file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("no stored sessions yet")]
    NoSessions,
    #[error("session log entry failed to parse: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::provider::{ContentBlock, Role};

    #[test]
    fn create_flush_load_roundtrips_messages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = Session::create(dir.path()).expect("create");
        session.messages.push(Message::user("hello"));
        session.messages.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text("hi".into())],
        });
        session.flush_from(0).expect("flush");
        session.messages.push(Message::user("again"));
        session.flush_from(2).expect("flush again");

        let loaded = Session::load(dir.path(), session.id()).expect("load");
        assert_eq!(loaded.messages.len(), 3);
        assert_eq!(loaded.messages[0], Message::user("hello"));
        assert_eq!(loaded.messages[1].text(), "hi");
        assert_eq!(loaded.messages[2], Message::user("again"));

        let latest = Session::load(dir.path(), "latest").expect("latest");
        assert_eq!(latest.id(), session.id());
        assert_eq!(Session::list(dir.path()).expect("list"), vec![session.id()]);
    }

    #[test]
    fn civil_dates_match_known_epochs() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(18_991), (2021, 12, 30));
        assert_eq!(civil_from_days(19_007), (2022, 1, 15));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
