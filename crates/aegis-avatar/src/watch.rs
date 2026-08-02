//! Debounced filesystem observation for transactional avatar reloads.
//!
//! The watcher reports only that source state may have changed. It never
//! decodes media or touches Flux from notify's callback thread: callers poll
//! on their render thread, build a complete replacement with
//! [`crate::Avatar::load_transactional`], and swap only after that succeeds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

const DEBOUNCE: Duration = Duration::from_millis(180);
const RETRY_DELAY: Duration = Duration::from_millis(350);
const MAX_RETRIES: u8 = 3;

/// Failure to create or arm avatar filesystem observation.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("create avatar filesystem watcher")]
    Create(#[source] notify::Error),
    #[error("no avatar source directory could be watched")]
    NoTargets,
}

#[derive(Clone, Debug)]
struct RelevantPaths {
    trees: Vec<PathBuf>,
    exact: Vec<PathBuf>,
}

impl RelevantPaths {
    fn current() -> Self {
        let mut trees = crate::vrm_candidate_paths()
            .into_iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        trees.sort();
        trees.dedup();

        let mut exact = crate::candidate_paths();
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

    fn is_pending(&self) -> bool {
        self.deadline.is_some()
    }
}

/// Watches every source that can affect [`crate::Avatar::load`].
///
/// Event callbacks only set an atomic dirty flag. [`Self::poll`] performs
/// trailing-edge debouncing on the caller's thread; a `true` result authorizes
/// one transactional reload attempt. Call [`Self::retry`] after a failed load
/// to tolerate editors that expose a partial file between rename/write events.
pub struct AvatarWatcher {
    watcher: RecommendedWatcher,
    relevant: Arc<RelevantPaths>,
    changed: Arc<AtomicBool>,
    watched: HashMap<PathBuf, WatchDepth>,
    schedule: ReloadSchedule,
}

impl AvatarWatcher {
    /// Create and arm a watcher for the current XDG/debug avatar resolution
    /// paths. Watcher setup is independent of whether an avatar exists yet.
    pub fn new() -> Result<Self, WatchError> {
        Self::from_relevant(RelevantPaths::current())
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
                        if matches!(event.kind, EventKind::Access(_)) {
                            return;
                        }
                        if event.paths.is_empty()
                            || event
                                .paths
                                .iter()
                                .any(|path| callback_relevant.includes(path))
                        {
                            callback_changed.store(true, Ordering::Release);
                        }
                    }
                    Err(error) => {
                        log::warn!("avatar: filesystem watcher error: {error}");
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

    /// Reconcile watches after a directory is created, removed, or replaced.
    /// Parent guards remain armed so a deleted avatar tree can reappear.
    pub fn refresh(&mut self) -> Result<(), WatchError> {
        let desired = watch_targets(&self.relevant);
        // Re-arm every target after a settled event. A directory can be
        // atomically replaced at the same pathname while the old watch stays
        // attached to the unlinked inode; comparing paths alone cannot detect
        // that case.
        for path in std::mem::take(&mut self.watched).into_keys() {
            if let Err(error) = self.watcher.unwatch(&path) {
                log::debug!("avatar: could not remove stale watch {:?}: {error}", path);
            }
        }

        for (path, depth) in desired {
            match self.watcher.watch(&path, depth.notify_mode()) {
                Ok(()) => {
                    self.watched.insert(path, depth);
                }
                Err(error) => {
                    log::warn!("avatar: could not watch {:?}: {error}", path);
                }
            }
        }
        if self.watched.is_empty() {
            Err(WatchError::NoTargets)
        } else {
            Ok(())
        }
    }

    /// Whether an event or debounce/retry deadline needs render-loop polling.
    #[must_use]
    pub fn needs_poll(&self) -> bool {
        self.changed.load(Ordering::Acquire) || self.schedule.is_pending()
    }

    /// Consume new events, apply trailing-edge debounce, and report when the
    /// caller should attempt one complete avatar rebuild.
    pub fn poll(&mut self) -> bool {
        let now = Instant::now();
        if self.changed.swap(false, Ordering::AcqRel) {
            self.schedule.observe(now);
        }
        self.schedule.take_ready(now)
    }

    /// Schedule a bounded retry after a failed transactional load.
    /// Returns `false` after the retry budget is exhausted; a later filesystem
    /// event starts a fresh budget.
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
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn relevant_paths_ignore_unrelated_home_changes() {
        let relevant = RelevantPaths {
            trees: vec![PathBuf::from("/data/aegis/avatars")],
            exact: vec![PathBuf::from("/home/test/.face")],
        };
        assert!(relevant.includes(Path::new("/data/aegis/avatars/motions/idle/a.vrma")));
        assert!(relevant.includes(Path::new("/data/aegis/avatars")));
        assert!(relevant.includes(Path::new("/home/test/.face")));
        assert!(!relevant.includes(Path::new("/home/test/Downloads/photo.png")));
        assert!(!relevant.includes(Path::new("/data/applications/app.desktop")));
    }

    #[test]
    fn schedule_is_trailing_edge_debounced_and_retries_are_bounded() {
        let start = Instant::now();
        let mut schedule = ReloadSchedule::default();
        schedule.observe(start);
        assert!(!schedule.take_ready(start + DEBOUNCE - Duration::from_millis(1)));
        schedule.observe(start + Duration::from_millis(100));
        assert!(!schedule.take_ready(start + DEBOUNCE));
        assert!(schedule.take_ready(start + Duration::from_millis(100) + DEBOUNCE));
        for retry in 0..MAX_RETRIES {
            assert!(schedule.retry(start + Duration::from_secs(u64::from(retry))));
        }
        assert!(!schedule.retry(start + Duration::from_secs(10)));
    }

    #[test]
    fn existing_tree_gets_recursive_watch_and_parent_guard() {
        let root = tempdir().unwrap();
        let tree = root.path().join("aegis/avatars");
        std::fs::create_dir_all(&tree).unwrap();
        let relevant = RelevantPaths {
            trees: vec![tree.clone()],
            exact: Vec::new(),
        };
        let targets = watch_targets(&relevant);
        assert_eq!(targets.get(&tree), Some(&WatchDepth::Recursive));
        assert_eq!(
            targets.get(tree.parent().unwrap()),
            Some(&WatchDepth::Direct)
        );
    }

    #[test]
    fn recursive_source_change_reaches_the_debounced_poll() {
        let root = tempdir().unwrap();
        let tree = root.path().join("aegis/avatars");
        std::fs::create_dir_all(&tree).unwrap();
        let relevant = RelevantPaths {
            trees: vec![tree.clone()],
            exact: Vec::new(),
        };
        let mut watcher = AvatarWatcher::from_relevant(relevant).unwrap();
        watcher.refresh().unwrap();

        std::fs::write(tree.join("avatar.vrm"), b"replacement").unwrap();
        let timeout = Instant::now() + Duration::from_secs(5);
        while Instant::now() < timeout {
            if watcher.poll() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("filesystem change did not reach the reload poll before timeout");
    }
}
