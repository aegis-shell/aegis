//! VRM Animation library discovery and playback policy.
//!
//! A motion library separates continuously selected idle clips from explicit
//! one-shot actions:
//!
//! - `motions/idle/*.vrma` forms a shuffled playlist. Every clip plays once
//!   before the bag is refilled, and a refill never immediately repeats the
//!   clip that ended the previous bag.
//! - `motions/actions/*.vrma` is addressable by file stem and can also be
//!   selected from an independent shuffle bag on request.
//! - `avatar.vrma` remains a single looping-idle compatibility source, but is
//!   ignored as soon as either library directory contains a clip.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use flux_scene_graph::{Animation, Scene};

use crate::vrm::VrmError;

/// The playback role assigned by a motion's directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionKind {
    /// Automatically selected from a non-repeating shuffled playlist.
    Idle,
    /// Played only after a named or random action request.
    Action,
}

/// Public metadata for one loaded VRM Animation clip.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionInfo {
    /// Stable request name, derived from the `.vrma` file stem.
    pub name: String,
    /// Whether the clip belongs to the idle or action pool.
    pub kind: MotionKind,
    /// Clip duration reported by the scene-graph animation loader.
    pub duration_seconds: f32,
}

#[derive(Clone, Debug)]
struct MotionSource {
    name: String,
    kind: MotionKind,
    path: PathBuf,
}

struct MotionClip {
    source: MotionSource,
    animation: Animation,
}

#[derive(Clone, Copy, Debug)]
struct Playback {
    clip: usize,
    elapsed: f32,
}

/// A shuffle bag guarantees full-pool coverage before refill and avoids the
/// boundary repeat that a naive random index commonly produces.
struct ShuffleBag {
    members: Vec<usize>,
    remaining: Vec<usize>,
    last: Option<usize>,
    rng: fastrand::Rng,
}

impl ShuffleBag {
    fn new(members: Vec<usize>) -> Self {
        Self::with_rng(members, fastrand::Rng::new())
    }

    #[cfg(test)]
    fn with_seed(members: Vec<usize>, seed: u64) -> Self {
        Self::with_rng(members, fastrand::Rng::with_seed(seed))
    }

    fn with_rng(members: Vec<usize>, rng: fastrand::Rng) -> Self {
        Self {
            members,
            remaining: Vec::new(),
            last: None,
            rng,
        }
    }

    fn next(&mut self) -> Option<usize> {
        if self.remaining.is_empty() {
            self.remaining.clone_from(&self.members);
            self.rng.shuffle(&mut self.remaining);
            let final_index = self.remaining.len().checked_sub(1)?;
            if self.remaining.len() > 1
                && self
                    .last
                    .is_some_and(|last| self.remaining[final_index] == last)
            {
                self.remaining.swap(0, final_index);
            }
        }
        let selected = self.remaining.pop()?;
        self.last = Some(selected);
        Some(selected)
    }
}

pub(crate) struct MotionLibrary {
    clips: Vec<MotionClip>,
    by_name: HashMap<String, usize>,
    idle: ShuffleBag,
    actions: ShuffleBag,
    current: Option<Playback>,
}

impl MotionLibrary {
    pub(crate) fn load(
        scene: &Scene,
        avatar_dir: &Path,
        legacy_path: Option<&Path>,
    ) -> Result<Self, VrmError> {
        let sources = discover_sources(avatar_dir, legacy_path)?;
        let mut clips: Vec<MotionClip> = Vec::with_capacity(sources.len());
        let mut by_name: HashMap<String, usize> = HashMap::with_capacity(sources.len());
        for source in sources {
            let bytes = std::fs::read(&source.path)
                .map_err(|error| VrmError::Io(source.path.clone(), error))?;
            let animation = scene
                .animation_from_glb(&bytes)
                .map_err(|error| VrmError::Animation(source.path.clone(), error))?;
            let duration = animation.duration();
            if !duration.is_finite() || duration <= f32::EPSILON {
                return Err(VrmError::MotionDuration(source.path, duration));
            }
            let channels = animation.channel_count();
            if channels == 0 {
                return Err(VrmError::MotionChannels(source.path));
            }
            log::info!(
                "avatar: loaded {:?} motion {:?}: {:.3}s, {} retargeted channels",
                source.kind,
                source.path,
                duration,
                channels
            );
            let index = clips.len();
            by_name.insert(source.name.clone(), index);
            clips.push(MotionClip { source, animation });
        }

        let idle = clips
            .iter()
            .enumerate()
            .filter_map(|(index, clip)| (clip.source.kind == MotionKind::Idle).then_some(index))
            .collect();
        let actions = clips
            .iter()
            .enumerate()
            .filter_map(|(index, clip)| (clip.source.kind == MotionKind::Action).then_some(index))
            .collect();
        let mut library = Self {
            clips,
            by_name,
            idle: ShuffleBag::new(idle),
            actions: ShuffleBag::new(actions),
            current: None,
        };
        library.select_next_idle();
        Ok(library)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    pub(crate) fn is_playing(&self) -> bool {
        self.current.is_some()
    }

    pub(crate) fn infos(&self) -> Vec<MotionInfo> {
        self.clips
            .iter()
            .map(|clip| MotionInfo {
                name: clip.source.name.clone(),
                kind: clip.source.kind,
                duration_seconds: clip.animation.duration(),
            })
            .collect()
    }

    pub(crate) fn current_name(&self) -> Option<&str> {
        self.current
            .map(|playback| self.clips[playback.clip].source.name.as_str())
    }

    pub(crate) fn play(&mut self, name: &str) -> bool {
        let Some(&clip) = self.by_name.get(name) else {
            return false;
        };
        self.current = Some(Playback { clip, elapsed: 0.0 });
        true
    }

    pub(crate) fn play_random_action(&mut self) -> Option<&str> {
        let clip = self.actions.next()?;
        self.current = Some(Playback { clip, elapsed: 0.0 });
        Some(self.clips[clip].source.name.as_str())
    }

    /// Advance the current clip. Every clip is sampled non-looping; crossing
    /// its end selects the next shuffled idle or resets the model to rest when
    /// no idle pool exists. Large frame deltas carry into the next clip.
    pub(crate) fn advance(&mut self, delta_seconds: f32) -> bool {
        if self.current.is_none() {
            return false;
        }
        let mut remaining = if delta_seconds.is_finite() {
            delta_seconds.max(0.0)
        } else {
            0.0
        };
        // Malformed zero-duration clips must not create an unbounded loop.
        let transition_budget = self.clips.len().max(1) + 1;
        for _ in 0..transition_budget {
            let playback = self.current.expect("checked above");
            let duration = self.clips[playback.clip].animation.duration().max(0.0);
            if duration <= f32::EPSILON {
                self.select_next_idle();
                if self.current.is_none() {
                    return true;
                }
                continue;
            }
            let until_end = (duration - playback.elapsed).max(0.0);
            if remaining < until_end {
                self.current.as_mut().expect("still selected").elapsed += remaining;
                return true;
            }
            remaining = (remaining - until_end).max(0.0);
            self.select_next_idle();
            if self.current.is_none() || remaining <= f32::EPSILON {
                return true;
            }
        }
        true
    }

    pub(crate) fn sample(&self) -> Option<(&Animation, f32, &Path)> {
        let playback = self.current?;
        let clip = &self.clips[playback.clip];
        Some((
            &clip.animation,
            playback.elapsed,
            clip.source.path.as_path(),
        ))
    }

    fn select_next_idle(&mut self) {
        self.current = self.idle.next().map(|clip| Playback { clip, elapsed: 0.0 });
    }
}

fn discover_sources(
    avatar_dir: &Path,
    legacy_path: Option<&Path>,
) -> Result<Vec<MotionSource>, VrmError> {
    let motion_root = avatar_dir.join("motions");
    let mut sources = Vec::new();
    sources.extend(discover_pool(&motion_root.join("idle"), MotionKind::Idle)?);
    sources.extend(discover_pool(
        &motion_root.join("actions"),
        MotionKind::Action,
    )?);
    if sources.is_empty()
        && let Some(path) = legacy_path.filter(|path| path.is_file())
    {
        sources.push(MotionSource {
            name: "default".to_owned(),
            kind: MotionKind::Idle,
            path: path.to_path_buf(),
        });
    } else if !sources.is_empty()
        && let Some(path) = legacy_path.filter(|path| path.is_file())
    {
        log::info!("avatar: ignoring legacy VRMA {path:?}; the motions library is configured");
    }

    let mut by_name: HashMap<&str, &Path> = HashMap::with_capacity(sources.len());
    for source in &sources {
        if let Some(previous) = by_name.insert(&source.name, &source.path) {
            return Err(VrmError::DuplicateMotion(
                source.name.clone(),
                previous.to_path_buf(),
                source.path.clone(),
            ));
        }
    }
    Ok(sources)
}

fn discover_pool(directory: &Path, kind: MotionKind) -> Result<Vec<MotionSource>, VrmError> {
    let metadata = match std::fs::metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(VrmError::Io(directory.to_path_buf(), error)),
    };
    if !metadata.is_dir() {
        return Err(VrmError::MotionDirectory(directory.to_path_buf()));
    }
    let entries = std::fs::read_dir(directory)
        .map_err(|error| VrmError::Io(directory.to_path_buf(), error))?;
    let mut paths = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| VrmError::Io(directory.to_path_buf(), error))
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();

    paths
        .into_iter()
        .filter(|path| crate::is_vrma_path(path))
        .map(|path| {
            let metadata =
                std::fs::metadata(&path).map_err(|error| VrmError::Io(path.clone(), error))?;
            if !metadata.is_file() {
                return Err(VrmError::MotionFile(path));
            }
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|name| valid_motion_name(name))
                .ok_or_else(|| VrmError::MotionName(path.clone()))?;
            Ok(MotionSource {
                name: name.to_owned(),
                kind,
                path,
            })
        })
        .collect()
}

fn valid_motion_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
        && name.as_bytes()[0].is_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn shuffle_bag_covers_each_member_and_avoids_boundary_repeat() {
        let mut bag = ShuffleBag::with_seed(vec![0, 1, 2, 3], 7);
        let first_cycle = (0..4).map(|_| bag.next().unwrap()).collect::<Vec<_>>();
        let second_cycle = (0..4).map(|_| bag.next().unwrap()).collect::<Vec<_>>();
        let mut first_sorted = first_cycle.clone();
        let mut second_sorted = second_cycle.clone();
        first_sorted.sort_unstable();
        second_sorted.sort_unstable();
        assert_eq!(first_sorted, vec![0, 1, 2, 3]);
        assert_eq!(second_sorted, vec![0, 1, 2, 3]);
        assert_ne!(first_cycle[3], second_cycle[0]);
    }

    #[test]
    fn discovery_is_sorted_and_uses_directory_as_the_motion_kind() {
        let root = tempdir().unwrap();
        let directory = root.path().join("motions/idle");
        std::fs::create_dir_all(&directory).unwrap();
        File::create(directory.join("wave.vrma")).unwrap();
        File::create(directory.join("breathe.VRMA")).unwrap();
        File::create(directory.join("notes.txt")).unwrap();

        let sources = discover_pool(&directory, MotionKind::Idle).unwrap();
        assert_eq!(
            sources
                .iter()
                .map(|source| source.name.as_str())
                .collect::<Vec<_>>(),
            vec!["breathe", "wave"]
        );
        assert!(sources.iter().all(|source| source.kind == MotionKind::Idle));
    }

    #[test]
    fn library_sources_override_the_legacy_companion() {
        let root = tempdir().unwrap();
        let actions = root.path().join("motions/actions");
        std::fs::create_dir_all(&actions).unwrap();
        File::create(actions.join("greeting.vrma")).unwrap();
        let legacy = root.path().join("avatar.vrma");
        File::create(&legacy).unwrap();

        let sources = discover_sources(root.path(), Some(&legacy)).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "greeting");
        assert_eq!(sources[0].kind, MotionKind::Action);
    }

    #[test]
    fn duplicate_names_across_pools_are_rejected() {
        let root = tempdir().unwrap();
        let idle = root.path().join("motions/idle");
        let actions = root.path().join("motions/actions");
        std::fs::create_dir_all(&idle).unwrap();
        std::fs::create_dir_all(&actions).unwrap();
        File::create(idle.join("wave.vrma")).unwrap();
        File::create(actions.join("wave.vrma")).unwrap();

        assert!(matches!(
            discover_sources(root.path(), None),
            Err(VrmError::DuplicateMotion(name, _, _)) if name == "wave"
        ));
    }

    #[test]
    fn public_motion_names_are_stable_ascii_identifiers() {
        for valid in ["idle", "greeting-2", "peace_sign"] {
            assert!(valid_motion_name(valid), "{valid}");
        }
        for invalid in ["", "Greeting", "2fast", "hello world", "招手"] {
            assert!(!valid_motion_name(invalid), "{invalid}");
        }
    }

    #[test]
    fn vrma_named_directory_is_rejected_as_a_clip() {
        let root = tempdir().unwrap();
        let directory = root.path().join("motions/actions");
        std::fs::create_dir_all(directory.join("wave.vrma")).unwrap();

        assert!(matches!(
            discover_pool(&directory, MotionKind::Action),
            Err(VrmError::MotionFile(path)) if path.ends_with("wave.vrma")
        ));
    }
}
