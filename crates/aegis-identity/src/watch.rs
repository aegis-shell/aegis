//! Debounced observation for the shared portrait configuration.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{PortraitCandidate, PortraitConfig};

const DEBOUNCE: Duration = Duration::from_millis(180);
const RETRY_DELAY: Duration = Duration::from_millis(350);
const MAX_RETRIES: u8 = 3;

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("create identity portrait filesystem watcher")]
    Create(#[source] notify::Error),
    #[error("no identity portrait source directory could be watched")]
    NoTargets,
}

#[derive(Clone, Debug)]
struct RelevantPaths {
    trees: Vec<PathBuf>,
    exact: Vec<PathBuf>,
}

impl RelevantPaths {
    fn from_config(config: &PortraitConfig) -> Self {
        let mut trees = Vec::new();
        let mut exact = Vec::new();
        for candidate in config.candidates() {
            match candidate {
                PortraitCandidate::Still(path) => exact.push(path.clone()),
                PortraitCandidate::Vrm { model, .. } => {
                    if let Some(parent) = model.parent() {
                        trees.push(parent.to_path_buf());
                    }
                }
            }
        }
        trees.sort();
        trees.dedup();
        exact.retain(|path| !trees.iter().any(|tree| path.starts_with(tree)));
        exact.sort();
        exact.dedup();
        Self { trees, exact }
    }

    fn includes(&self, path: &Path) -> bool {
        self.trees
            .iter()
            .any(|tree| path.starts_with(tree) || tree.starts_with(path))
            || self
                .exact
                .iter()
                .any(|exact| path == exact || exact.starts_with(path))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchDepth {
    Direct,
    Recursive,
}

impl WatchDepth {
    fn notify_mode(self) -> RecursiveMode {
        match self {
            Self::Direct => RecursiveMode::NonRecursive,
            Self::Recursive => RecursiveMode::Recursive,
        }
    }
}

#[derive(Debug, Default)]
struct ReloadSchedule {
    deadline: Option<Instant>,
    retries_remaining: u8,
}

impl ReloadSchedule {
    fn observe(&mut self, now: Instant) {
        self.deadline = Some(now + DEBOUNCE);
        self.retries_remaining = MAX_RETRIES;
    }

    fn take_ready(&mut self, now: Instant) -> bool {
        if self.deadline.is_some_and(|deadline| deadline <= now) {
            self.deadline = None;
            true
        } else {
            false
        }
    }

    fn retry(&mut self, now: Instant) -> bool {
        if self.retries_remaining == 0 {
            return false;
        }
        self.retries_remaining -= 1;
        self.deadline = Some(now + RETRY_DELAY);
        true
    }
}

/// Watches every source described by one immutable [`PortraitConfig`].
pub struct PortraitWatcher {
    watcher: RecommendedWatcher,
    relevant: Arc<RelevantPaths>,
    changed: Arc<AtomicBool>,
    watched: HashMap<PathBuf, WatchDepth>,
    schedule: ReloadSchedule,
}

impl PortraitWatcher {
    pub fn new(config: &PortraitConfig) -> Result<Self, WatchError> {
        Self::from_relevant(RelevantPaths::from_config(config))
    }

    fn from_relevant(relevant: RelevantPaths) -> Result<Self, WatchError> {
        let relevant = Arc::new(relevant);
        let changed = Arc::new(AtomicBool::new(false));
        let callback_relevant = Arc::clone(&relevant);
        let callback_changed = Arc::clone(&changed);
        let watcher =
            notify::recommended_watcher(
                move |result: notify::Result<notify::Event>| match result {
                    Ok(event) => {
                        if !matches!(event.kind, EventKind::Access(_))
                            && (event.paths.is_empty()
                                || event
                                    .paths
                                    .iter()
                                    .any(|path| callback_relevant.includes(path)))
                        {
                            callback_changed.store(true, Ordering::Release);
                        }
                    }
                    Err(error) => {
                        log::warn!("identity portrait: filesystem watcher error: {error}");
                        callback_changed.store(true, Ordering::Release);
                    }
                },
            )
            .map_err(WatchError::Create)?;
        let mut this = Self {
            watcher,
            relevant,
            changed,
            watched: HashMap::new(),
            schedule: ReloadSchedule::default(),
        };
        this.refresh()?;
        Ok(this)
    }

    pub fn refresh(&mut self) -> Result<(), WatchError> {
        let desired = watch_targets(&self.relevant);
        for path in std::mem::take(&mut self.watched).into_keys() {
            if let Err(error) = self.watcher.unwatch(&path) {
                log::debug!("identity portrait: could not remove stale watch {path:?}: {error}");
            }
        }
        for (path, depth) in desired {
            match self.watcher.watch(&path, depth.notify_mode()) {
                Ok(()) => {
                    self.watched.insert(path, depth);
                }
                Err(error) => {
                    log::warn!("identity portrait: could not watch {path:?}: {error}");
                }
            }
        }
        if self.watched.is_empty() {
            Err(WatchError::NoTargets)
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub fn needs_poll(&self) -> bool {
        self.changed.load(Ordering::Acquire) || self.schedule.deadline.is_some()
    }

    pub fn poll(&mut self) -> bool {
        let now = Instant::now();
        if self.changed.swap(false, Ordering::AcqRel) {
            self.schedule.observe(now);
        }
        self.schedule.take_ready(now)
    }

    pub fn retry(&mut self) -> bool {
        self.schedule.retry(Instant::now())
    }
}

fn watch_targets(relevant: &RelevantPaths) -> HashMap<PathBuf, WatchDepth> {
    let mut targets = HashMap::new();
    for tree in &relevant.trees {
        if tree.is_dir() {
            insert_target(&mut targets, tree.clone(), WatchDepth::Recursive);
            if let Some(parent) = tree.parent().filter(|parent| parent.is_dir()) {
                insert_target(&mut targets, parent.to_path_buf(), WatchDepth::Direct);
            }
        } else if let Some(existing) = nearest_existing_directory(tree) {
            insert_target(&mut targets, existing, WatchDepth::Direct);
        }
    }
    for path in &relevant.exact {
        if let Some(parent) = path.parent().and_then(nearest_existing_directory) {
            insert_target(&mut targets, parent, WatchDepth::Direct);
        }
    }
    targets
}

fn insert_target(targets: &mut HashMap<PathBuf, WatchDepth>, path: PathBuf, depth: WatchDepth) {
    targets
        .entry(path)
        .and_modify(|current| {
            if depth == WatchDepth::Recursive {
                *current = depth;
            }
        })
        .or_insert(depth);
}

fn nearest_existing_directory(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|path| path.is_dir())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_config_drives_still_and_motion_observation() {
        let relevant = RelevantPaths::from_config(&PortraitConfig::new(vec![
            PortraitCandidate::Still(PathBuf::from("/home/test/.face")),
            PortraitCandidate::Vrm {
                model: PathBuf::from("/data/aegis/avatars/avatar.vrm"),
                legacy_motion: PathBuf::from("/data/aegis/avatars/avatar.vrma"),
            },
        ]));
        assert!(relevant.includes(Path::new("/home/test/.face")));
        assert!(relevant.includes(Path::new(
            "/data/aegis/avatars/motions/actions/greeting.vrma"
        )));
        assert!(!relevant.includes(Path::new("/home/test/Downloads/photo.png")));
    }

    #[test]
    fn retry_schedule_is_bounded() {
        let start = Instant::now();
        let mut schedule = ReloadSchedule::default();
        schedule.observe(start);
        assert!(!schedule.take_ready(start + DEBOUNCE - Duration::from_millis(1)));
        assert!(schedule.take_ready(start + DEBOUNCE));
        for retry in 0..MAX_RETRIES {
            assert!(schedule.retry(start + Duration::from_secs(u64::from(retry))));
        }
        assert!(!schedule.retry(start + Duration::from_secs(10)));
    }
}
