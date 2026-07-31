//! The file picker's pure state machine — directory listing, filtering,
//! selection, and confirmation outcomes.
//!
//! No flux, lens, or Wayland dependency. The chrome component in
//! `aegis-shell` owns one of these and delegates key handling and list
//! state to it; this module is unit-tested in isolation, mirroring the
//! launcher brain (ADR-0022). It backs the user-consent file pick (the
//! compositor side of the FileChooser portal); unlike the target pick
//! (ADR-0054) it never freezes the screen or reads screen content.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::input::KeyAction;

/// What a file pick asks the user for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePickMode {
    /// Pick an existing file (or several, with `multiple`).
    Open,
    /// Name a file to write; the target may not exist yet.
    Save,
    /// Pick a directory.
    ChooseDir,
}

/// One named filter of selectable files. `patterns` are globs (`"*.png"`)
/// or MIME types (`"image/png"`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub label: String,
    pub patterns: Vec<String>,
}

impl Filter {
    /// Whether `file_name` passes this filter. An empty pattern list matches
    /// everything (an "All files" row).
    pub fn matches(&self, file_name: &str) -> bool {
        self.patterns.is_empty()
            || self
                .patterns
                .iter()
                .any(|pattern| pattern_matches(pattern, file_name))
    }
}

/// One directory entry in the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// What [`FilePickerModel::confirm`] resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// The user confirmed these paths (files or directories).
    Paths(Vec<PathBuf>),
    /// The highlighted row was a directory; the model navigated into it
    /// instead of confirming.
    Navigated,
    /// Nothing to confirm: no highlighted row, or a Save with an empty
    /// filename.
    Ignored,
}

/// (extension, MIME type) pairs the picker can resolve. Deliberately small:
/// an unknown extension or MIME pattern matches nothing, because fail-closed
/// beats offering the user files the requesting application did not ask for.
const MIME_BY_EXTENSION: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("txt", "text/plain"),
    ("md", "text/markdown"),
    ("pdf", "application/pdf"),
    ("mp3", "audio/mpeg"),
    ("ogg", "audio/ogg"),
    ("wav", "audio/wav"),
    ("flac", "audio/flac"),
    ("mp4", "video/mp4"),
    ("mkv", "video/x-matroska"),
    ("webm", "video/webm"),
];

fn pattern_matches(pattern: &str, file_name: &str) -> bool {
    if pattern.contains('/') {
        mime_matches(pattern, file_name)
    } else {
        // Glob: `*.png` matches by case-insensitive suffix; a bare `*`
        // matches everything.
        let suffix = pattern.trim_start_matches('*').to_lowercase();
        suffix.is_empty() || file_name.to_lowercase().ends_with(&suffix)
    }
}

fn mime_matches(pattern: &str, file_name: &str) -> bool {
    let Some((_, extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    let extension = extension.to_lowercase();
    let Some((_, mime)) = MIME_BY_EXTENSION.iter().find(|(ext, _)| *ext == extension) else {
        return false;
    };
    if let Some(prefix) = pattern.strip_suffix('*') {
        // A wildcard such as `image/*`.
        return mime.starts_with(prefix);
    }
    pattern.eq_ignore_ascii_case(mime)
}

/// The file picker's interaction state.
///
/// Holds the current directory's listing plus the selection model the
/// chrome renders. `selected` is the single highlighted row (an index into
/// [`FilePickerModel::entries`]); `marked` collects the multi-selection as
/// entry indices so it survives filter and hidden-file changes. Directory
/// reads are synchronous — acceptable for the v1 picker because the model
/// only reads on navigation, never per frame.
pub struct FilePickerModel {
    mode: FilePickMode,
    multiple: bool,
    /// Effective directory-pick flag: set for `ChooseDir` or an explicit
    /// `directory` request. The listing shows directories only and confirm
    /// resolves to a directory.
    directory: bool,
    dir: PathBuf,
    entries: Vec<Entry>,
    selected: Option<usize>,
    marked: BTreeSet<usize>,
    /// The Save-mode edit buffer, seeded from the suggested name.
    filename: String,
    filters: Vec<Filter>,
    active_filter: usize,
    show_hidden: bool,
    /// Last failed directory read, surfaced for the chrome to display.
    error: Option<String>,
}

impl FilePickerModel {
    /// Construct a model rooted at `start_dir` and read its listing.
    pub fn new(
        mode: FilePickMode,
        multiple: bool,
        directory: bool,
        start_dir: PathBuf,
        suggested_name: Option<String>,
        filters: Vec<Filter>,
    ) -> FilePickerModel {
        let mut model = FilePickerModel {
            mode,
            multiple,
            directory: directory || mode == FilePickMode::ChooseDir,
            dir: PathBuf::new(),
            entries: Vec::new(),
            selected: None,
            marked: BTreeSet::new(),
            filename: suggested_name.unwrap_or_default(),
            filters,
            active_filter: 0,
            show_hidden: false,
            error: None,
        };
        model.load(start_dir);
        model
    }

    pub fn mode(&self) -> FilePickMode {
        self.mode
    }

    pub fn multiple(&self) -> bool {
        self.multiple
    }

    /// Whether this pick chooses directories (an explicit request or
    /// `ChooseDir` mode).
    pub fn directory(&self) -> bool {
        self.directory
    }

    /// The directory the listing belongs to.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The full listing: directories first, then files, each group sorted
    /// by case-insensitive name.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The highlighted row as an index into [`FilePickerModel::entries`].
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The multi-selection as entry indices (Open mode with `multiple`).
    pub fn marked(&self) -> &BTreeSet<usize> {
        &self.marked
    }

    pub fn is_marked(&self, entry_idx: usize) -> bool {
        self.marked.contains(&entry_idx)
    }

    /// The Save-mode filename edit buffer.
    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn filters(&self) -> &[Filter] {
        &self.filters
    }

    /// Index of the active filter into [`FilePickerModel::filters`].
    pub fn active_filter(&self) -> usize {
        self.active_filter
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// The last failed directory read, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Replace the listing with `dir`'s contents. On a read error the
    /// current directory is kept and the error string is surfaced through
    /// [`FilePickerModel::error`].
    pub fn load(&mut self, dir: PathBuf) {
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            Err(error) => {
                self.error = Some(format!("cannot open {}: {error}", dir.display()));
                return;
            }
        };
        let mut entries: Vec<Entry> = read
            .filter_map(|entry| entry.ok())
            .map(|entry| Entry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false),
            })
            .collect();
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.dir = dir;
        self.entries = entries;
        self.marked.clear();
        self.error = None;
        self.reset_selection();
    }

    /// Navigate to the parent directory. A no-op at the filesystem root.
    pub fn go_parent(&mut self) {
        if let Some(parent) = self.dir.parent() {
            self.load(parent.to_path_buf());
        }
    }

    /// Navigate into the highlighted directory. Returns whether the
    /// directory changed (a read error keeps the current one).
    pub fn enter_selected_dir(&mut self) -> bool {
        let Some(idx) = self.selected else {
            return false;
        };
        if !self.entries[idx].is_dir {
            return false;
        }
        let before = self.dir.clone();
        self.load(before.join(&self.entries[idx].name));
        self.dir != before
    }

    /// Move the highlight by `delta` rows through the filtered view,
    /// wrapping at both ends (the launcher brain's behavior).
    pub fn move_selection(&mut self, delta: i32) {
        let filtered = self.filtered_indices();
        if filtered.is_empty() {
            self.selected = None;
            return;
        }
        let position = self
            .selected
            .and_then(|idx| filtered.iter().position(|&i| i == idx))
            .unwrap_or(0) as i32;
        let next = (position + delta).rem_euclid(filtered.len() as i32) as usize;
        self.selected = Some(filtered[next]);
    }

    /// Highlight `entry_idx` directly (a pointer click on a row).
    pub fn select(&mut self, entry_idx: usize) {
        if entry_idx < self.entries.len() {
            self.selected = Some(entry_idx);
        }
    }

    /// Toggle the highlighted file's mark in the multi-selection. No-op
    /// outside multi-file Open mode and on directories.
    pub fn toggle_mark(&mut self) {
        if !self.multiple || self.directory {
            return;
        }
        let Some(idx) = self.selected else {
            return;
        };
        if self.entries[idx].is_dir {
            return;
        }
        if !self.marked.remove(&idx) {
            self.marked.insert(idx);
        }
    }

    /// Edit the Save-mode filename buffer with one key action (`Char`
    /// appends, `Backspace` pops). No-op in other modes; the chrome routes
    /// navigation and confirmation keys itself.
    pub fn apply_key(&mut self, action: KeyAction) {
        if self.mode != FilePickMode::Save {
            return;
        }
        match action {
            KeyAction::Char(c) => self.filename.push(c),
            KeyAction::Backspace => {
                self.filename.pop();
            }
            _ => {}
        }
    }

    /// Toggle dotfile visibility and re-seat the highlight.
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.reset_selection();
    }

    /// Activate the next/previous filter, wrapping. No-op without filters.
    pub fn cycle_filter(&mut self, delta: i32) {
        if self.filters.is_empty() {
            return;
        }
        let len = self.filters.len() as i32;
        self.active_filter = ((self.active_filter as i32 + delta).rem_euclid(len)) as usize;
        self.reset_selection();
    }

    /// Entry indices passing the active filter, the hidden-file rule, and
    /// the directory-mode rule, in display order. Directory picks list
    /// directories only; file picks list every directory (for navigation)
    /// plus the files the active filter accepts.
    pub fn filtered_indices(&self) -> Vec<usize> {
        let filter = self.filters.get(self.active_filter);
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                if !self.show_hidden && entry.name.starts_with('.') {
                    return false;
                }
                if self.directory {
                    return entry.is_dir;
                }
                entry.is_dir || filter.is_none_or(|filter| filter.matches(&entry.name))
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Resolve the user's confirmation. Open single resolves the highlighted
    /// file (or navigates into a highlighted directory); Open multiple
    /// resolves the marked files, falling back to the single behavior when
    /// nothing is marked; directory picks resolve the highlighted directory
    /// or, with nothing highlighted, the current directory; Save resolves
    /// the current directory plus the filename buffer and refuses an empty
    /// one.
    pub fn confirm(&mut self) -> ConfirmOutcome {
        match self.mode {
            FilePickMode::Save => {
                let name = self.filename.trim();
                if name.is_empty() {
                    return ConfirmOutcome::Ignored;
                }
                ConfirmOutcome::Paths(vec![self.dir.join(name)])
            }
            _ if self.directory => ConfirmOutcome::Paths(vec![self.chosen_dir()]),
            FilePickMode::Open => {
                if self.multiple && !self.marked.is_empty() {
                    let paths = self
                        .marked
                        .iter()
                        .map(|&idx| self.dir.join(&self.entries[idx].name))
                        .collect();
                    return ConfirmOutcome::Paths(paths);
                }
                match self.selected {
                    Some(idx) if self.entries[idx].is_dir => {
                        let target = self.dir.join(&self.entries[idx].name);
                        self.load(target);
                        ConfirmOutcome::Navigated
                    }
                    Some(idx) => {
                        ConfirmOutcome::Paths(vec![self.dir.join(&self.entries[idx].name)])
                    }
                    None => ConfirmOutcome::Ignored,
                }
            }
            // Unreachable: `directory` is always set for `ChooseDir`.
            FilePickMode::ChooseDir => ConfirmOutcome::Ignored,
        }
    }

    /// The directory a directory-mode confirm resolves to: the highlighted
    /// directory when there is one, else the current directory.
    fn chosen_dir(&self) -> PathBuf {
        match self.selected {
            Some(idx) if self.entries[idx].is_dir => self.dir.join(&self.entries[idx].name),
            _ => self.dir.clone(),
        }
    }

    /// Seat the highlight on the first visible row after the visible set
    /// changed (navigation, filter cycle, hidden toggle).
    fn reset_selection(&mut self) {
        self.selected = self.filtered_indices().first().copied();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory with a fixed listing, namespaced by pid + counter
    /// so parallel tests do not collide. Removed on drop.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(files: &[&str], dirs: &[&str]) -> ScratchDir {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("aegis-file-picker-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            for dir in dirs {
                std::fs::create_dir_all(path.join(dir)).unwrap();
            }
            for file in files {
                std::fs::write(path.join(file), b"x").unwrap();
            }
            ScratchDir(path)
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn model(dir: PathBuf, mode: FilePickMode) -> FilePickerModel {
        FilePickerModel::new(mode, false, false, dir, None, Vec::new())
    }

    fn names(model: &FilePickerModel) -> Vec<&str> {
        model.entries().iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn entries_sort_dirs_first_then_case_insensitive_names() {
        let scratch = ScratchDir::new(&["b.txt", "A.png", "c.md"], &["zeta", "Alpha"]);
        let m = model(scratch.path(), FilePickMode::Open);
        assert_eq!(names(&m), ["Alpha", "zeta", "A.png", "b.txt", "c.md"]);
    }

    #[test]
    fn dotfiles_stay_hidden_until_toggled() {
        let scratch = ScratchDir::new(&["visible.txt", ".hidden"], &[]);
        let mut m = model(scratch.path(), FilePickMode::Open);
        assert_eq!(m.filtered_indices().len(), 1);
        m.toggle_hidden();
        assert!(m.show_hidden());
        assert_eq!(m.filtered_indices().len(), 2);
        m.toggle_hidden();
        assert_eq!(m.filtered_indices().len(), 1);
    }

    #[test]
    fn glob_patterns_match_by_case_insensitive_suffix() {
        let filter = Filter {
            label: "Images".into(),
            patterns: vec!["*.png".into()],
        };
        assert!(filter.matches("photo.png"));
        assert!(filter.matches("photo.PNG"));
        assert!(!filter.matches("photo.jpg"));
        assert!(!filter.matches("png.txt"));
        // A bare `*` matches everything; an empty pattern list is "All files".
        let any = Filter {
            label: "Any".into(),
            patterns: vec!["*".into()],
        };
        assert!(any.matches("anything.at.all"));
        assert!(Filter::default().matches("anything.at.all"));
    }

    #[test]
    fn mime_patterns_match_the_extension_table() {
        let png = Filter {
            label: "PNG".into(),
            patterns: vec!["image/png".into()],
        };
        assert!(png.matches("photo.png"));
        assert!(!png.matches("photo.jpg"));
        let images = Filter {
            label: "Images".into(),
            patterns: vec!["image/*".into()],
        };
        assert!(images.matches("a.png"));
        assert!(images.matches("b.jpeg"));
        assert!(images.matches("c.svg"));
        assert!(!images.matches("d.txt"));
        let videos = Filter {
            label: "Videos".into(),
            patterns: vec!["video/*".into()],
        };
        assert!(videos.matches("clip.mp4"));
        assert!(!videos.matches("song.mp3"));
        // Unknown extensions and unknown MIME types match nothing
        // (fail-closed beats over-matching).
        assert!(!images.matches("archive.xz"));
        let unknown = Filter {
            label: "Unknown".into(),
            patterns: vec!["application/x-tar".into()],
        };
        assert!(!unknown.matches("archive.tar"));
        // Extensionless files never match a MIME pattern.
        assert!(!png.matches("LICENSE"));
    }

    #[test]
    fn filters_gate_the_file_rows_but_not_directories() {
        let scratch = ScratchDir::new(&["a.png", "b.txt"], &["sub"]);
        let filters = vec![Filter {
            label: "Images".into(),
            patterns: vec!["*.png".into()],
        }];
        let mut m = FilePickerModel::new(
            FilePickMode::Open,
            false,
            false,
            scratch.path(),
            None,
            filters,
        );
        let visible: Vec<&str> = m
            .filtered_indices()
            .iter()
            .map(|&i| m.entries()[i].name.as_str())
            .collect();
        assert_eq!(visible, ["sub", "a.png"]);
        // Cycling back around restores the unfiltered view... with one
        // filter the cycle lands on itself; add a second to observe motion.
        assert_eq!(m.active_filter(), 0);
        m.cycle_filter(1);
        assert_eq!(m.active_filter(), 0, "a single filter wraps onto itself");
    }

    #[test]
    fn cycle_filter_moves_and_wraps() {
        let scratch = ScratchDir::new(&["a.png", "b.txt"], &[]);
        let filters = vec![
            Filter {
                label: "Images".into(),
                patterns: vec!["*.png".into()],
            },
            Filter {
                label: "Text".into(),
                patterns: vec!["*.txt".into()],
            },
        ];
        let mut m = FilePickerModel::new(
            FilePickMode::Open,
            false,
            false,
            scratch.path(),
            None,
            filters,
        );
        assert_eq!(m.filtered_indices().len(), 1);
        m.cycle_filter(1);
        assert_eq!(m.active_filter(), 1);
        assert_eq!(m.filtered_indices().len(), 1);
        assert_eq!(m.entries()[m.filtered_indices()[0]].name, "b.txt");
        m.cycle_filter(1);
        assert_eq!(m.active_filter(), 0, "wraps to the first filter");
        m.cycle_filter(-1);
        assert_eq!(m.active_filter(), 1, "wraps backwards");
    }

    #[test]
    fn selection_moves_with_wrap_and_skips_filtered_rows() {
        let scratch = ScratchDir::new(&["a.txt", "b.txt", "c.txt"], &[]);
        let mut m = model(scratch.path(), FilePickMode::Open);
        assert_eq!(m.selected(), Some(0));
        m.move_selection(-1);
        assert_eq!(m.selected(), Some(2), "wraps to the last row");
        m.move_selection(1);
        assert_eq!(m.selected(), Some(0));
        m.move_selection(2);
        assert_eq!(m.selected(), Some(2));
    }

    #[test]
    fn open_single_confirms_a_file_and_enters_a_directory() {
        let scratch = ScratchDir::new(&["a.txt"], &["sub"]);
        let mut m = model(scratch.path(), FilePickMode::Open);
        // The directory sorts first and is highlighted.
        assert_eq!(m.selected(), Some(0));
        assert!(m.entries()[0].is_dir);
        assert_eq!(m.confirm(), ConfirmOutcome::Navigated);
        assert_eq!(m.dir(), scratch.path().join("sub"));

        m.go_parent();
        assert_eq!(m.dir(), scratch.path());
        m.move_selection(1); // a.txt
        let expected = scratch.path().join("a.txt");
        assert_eq!(m.confirm(), ConfirmOutcome::Paths(vec![expected]));
    }

    #[test]
    fn open_multiple_confirms_marked_files() {
        let scratch = ScratchDir::new(&["a.txt", "b.txt", "c.txt"], &[]);
        let mut m = FilePickerModel::new(
            FilePickMode::Open,
            true,
            false,
            scratch.path(),
            None,
            Vec::new(),
        );
        m.toggle_mark(); // a.txt
        m.move_selection(2);
        m.toggle_mark(); // c.txt
        assert_eq!(m.marked().len(), 2);
        match m.confirm() {
            ConfirmOutcome::Paths(paths) => assert_eq!(
                paths,
                vec![scratch.path().join("a.txt"), scratch.path().join("c.txt")]
            ),
            other => panic!("expected Paths, got {other:?}"),
        }
    }

    #[test]
    fn open_multiple_without_marks_falls_back_to_the_highlighted_file() {
        let scratch = ScratchDir::new(&["a.txt", "b.txt"], &[]);
        let mut m = FilePickerModel::new(
            FilePickMode::Open,
            true,
            false,
            scratch.path(),
            None,
            Vec::new(),
        );
        m.move_selection(1);
        assert_eq!(
            m.confirm(),
            ConfirmOutcome::Paths(vec![scratch.path().join("b.txt")])
        );
    }

    #[test]
    fn marks_ignore_directories_and_single_open_mode() {
        let scratch = ScratchDir::new(&["a.txt"], &["sub"]);
        let mut m = FilePickerModel::new(
            FilePickMode::Open,
            true,
            false,
            scratch.path(),
            None,
            Vec::new(),
        );
        m.toggle_mark(); // highlighted row is a directory
        assert!(m.marked().is_empty());
        let mut single = model(scratch.path(), FilePickMode::Open);
        single.move_selection(1);
        single.toggle_mark();
        assert!(single.marked().is_empty(), "single-open never marks");
    }

    #[test]
    fn choose_dir_confirms_selected_or_current_directory() {
        let scratch = ScratchDir::new(&["a.txt"], &["sub"]);
        let mut m = model(scratch.path(), FilePickMode::ChooseDir);
        assert!(m.directory());
        // Only the directory is listed.
        assert_eq!(m.filtered_indices().len(), 1);
        assert_eq!(
            m.confirm(),
            ConfirmOutcome::Paths(vec![scratch.path().join("sub")])
        );
        // Into the subdirectory: nothing highlighted there, so confirm
        // resolves the current directory itself.
        assert!(m.enter_selected_dir());
        assert_eq!(m.selected(), None);
        assert_eq!(
            m.confirm(),
            ConfirmOutcome::Paths(vec![scratch.path().join("sub")])
        );
    }

    #[test]
    fn save_requires_a_filename_and_joins_the_current_dir() {
        let scratch = ScratchDir::new(&["existing.txt"], &[]);
        let mut m = FilePickerModel::new(
            FilePickMode::Save,
            false,
            false,
            scratch.path(),
            None,
            Vec::new(),
        );
        assert_eq!(
            m.confirm(),
            ConfirmOutcome::Ignored,
            "empty filename refuses"
        );
        for c in "out.txt".chars() {
            m.apply_key(KeyAction::Char(c));
        }
        assert_eq!(
            m.confirm(),
            ConfirmOutcome::Paths(vec![scratch.path().join("out.txt")])
        );
        // Backspace edits; the buffer survives navigation.
        m.apply_key(KeyAction::Backspace);
        assert_eq!(m.filename(), "out.tx");
        let mut seeded = FilePickerModel::new(
            FilePickMode::Save,
            false,
            false,
            scratch.path(),
            Some("seeded.png".into()),
            Vec::new(),
        );
        assert_eq!(seeded.filename(), "seeded.png");
        assert_eq!(
            seeded.confirm(),
            ConfirmOutcome::Paths(vec![scratch.path().join("seeded.png")])
        );
    }

    #[test]
    fn apply_key_is_a_noop_outside_save_mode() {
        let scratch = ScratchDir::new(&["a.txt"], &[]);
        let mut m = model(scratch.path(), FilePickMode::Open);
        m.apply_key(KeyAction::Char('x'));
        m.apply_key(KeyAction::Backspace);
        assert!(m.filename().is_empty());
    }

    #[test]
    fn parent_navigation_at_the_root_is_a_noop() {
        let mut m = model(PathBuf::from("/"), FilePickMode::Open);
        m.go_parent();
        assert_eq!(m.dir(), Path::new("/"));
    }

    #[test]
    fn failed_load_keeps_the_current_dir_and_surfaces_an_error() {
        let scratch = ScratchDir::new(&["a.txt"], &[]);
        let mut m = model(scratch.path(), FilePickMode::Open);
        m.load(scratch.path().join("does-not-exist"));
        assert_eq!(m.dir(), scratch.path());
        assert!(m.error().is_some());
        assert_eq!(m.entries().len(), 1, "the previous listing survives");
        // A successful load clears the error again.
        m.load(scratch.path());
        assert!(m.error().is_none());
    }
}
