//! Built-in tools: file reads/writes/edits, glob, grep, bounded shell, and
//! image loading. Paths are interpreted against fuji's working directory.

use std::path::{Path, PathBuf};
use std::pin::Pin;

use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Tool, ToolOutput};
use crate::agent::provider::ImageData;
use crate::agent::skills::Skill;

const READ_DEFAULT_LIMIT: usize = 1000;
const GLOB_CAP: usize = 100;
const GREP_DEFAULT_CAP: usize = 50;
const BASH_OUTPUT_CAP: usize = 30_000;
const BASH_DEFAULT_TIMEOUT: u64 = 120;
const BASH_MAX_TIMEOUT: u64 = 600;
const IMAGE_BYTE_CAP: u64 = 20 * 1024 * 1024;

type CallFuture<'a> = Pin<Box<dyn std::future::Future<Output = ToolOutput> + Send + 'a>>;

fn boxed<'a, F>(future: F) -> CallFuture<'a>
where
    F: std::future::Future<Output = ToolOutput> + Send + 'a,
{
    Box::pin(future)
}

/// Every built-in tool, `skill_read` bound to the discovered skills.
pub fn all(skills: Vec<Skill>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ReadFile),
        Box::new(WriteFile),
        Box::new(EditFile),
        Box::new(Glob),
        Box::new(Grep),
        Box::new(Bash),
        Box::new(ReadImage),
        Box::new(SkillRead::new(skills)),
    ]
}

fn parse_args<T: for<'de> Deserialize<'de>>(arguments: Value) -> Result<T, ToolOutput> {
    serde_json::from_value(arguments)
        .map_err(|error| ToolOutput::error(format!("invalid arguments: {error}")))
}

// ---------------------------------------------------------------- read_file

struct ReadFile;

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file. Returns up to `limit` lines starting at 1-based `offset` (defaults: whole file, at most 1000 lines)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1},
                "offset": {"type": "integer", "minimum": 1},
                "limit": {"type": "integer", "minimum": 1, "maximum": 2000},
            },
            "required": ["path"],
            "additionalProperties": false,
        })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, arguments: Value) -> CallFuture<'a> {
        boxed(async move {
            let args: ReadArgs = match parse_args(arguments) {
                Ok(args) => args,
                Err(output) => return output,
            };
            let text = match std::fs::read_to_string(&args.path) {
                Ok(text) => text,
                Err(error) => {
                    return ToolOutput::error(format!("cannot read {}: {error}", args.path));
                }
            };
            let offset = args.offset.unwrap_or(1).max(1);
            let limit = args.limit.unwrap_or(READ_DEFAULT_LIMIT);
            let lines: Vec<&str> = text.lines().collect();
            let total = lines.len();
            let slice: Vec<&str> = lines
                .iter()
                .skip(offset.saturating_sub(1))
                .take(limit)
                .copied()
                .collect();
            let mut out = slice.join("\n");
            if offset + slice.len() - 1 < total {
                out.push_str(&format!(
                    "\n[truncated: {} of {total} lines shown]",
                    slice.len()
                ));
            }
            ToolOutput::ok(out)
        })
    }
}

// --------------------------------------------------------------- write_file

struct WriteFile;

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Create or fully overwrite a file, creating parent directories as needed."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1},
                "content": {"type": "string"},
            },
            "required": ["path", "content"],
            "additionalProperties": false,
        })
    }

    fn call<'a>(&'a self, arguments: Value) -> CallFuture<'a> {
        boxed(async move {
            let args: WriteArgs = match parse_args(arguments) {
                Ok(args) => args,
                Err(output) => return output,
            };
            let path = Path::new(&args.path);
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                return ToolOutput::error(format!("cannot create {}: {error}", parent.display()));
            }
            match std::fs::write(path, &args.content) {
                Ok(()) => ToolOutput::ok(format!(
                    "wrote {} bytes to {}",
                    args.content.len(),
                    args.path
                )),
                Err(error) => ToolOutput::error(format!("cannot write {}: {error}", args.path)),
            }
        })
    }
}

// ---------------------------------------------------------------- edit_file

struct EditFile;

#[derive(Deserialize)]
struct EditArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace an exact string in a file. Fails when `old_string` is absent or matches more than once without `replace_all`."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1},
                "old_string": {"type": "string", "minLength": 1},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean"},
            },
            "required": ["path", "old_string", "new_string"],
            "additionalProperties": false,
        })
    }

    fn call<'a>(&'a self, arguments: Value) -> CallFuture<'a> {
        boxed(async move {
            let args: EditArgs = match parse_args(arguments) {
                Ok(args) => args,
                Err(output) => return output,
            };
            if args.old_string.is_empty() {
                return ToolOutput::error("old_string must not be empty");
            }
            let text = match std::fs::read_to_string(&args.path) {
                Ok(text) => text,
                Err(error) => {
                    return ToolOutput::error(format!("cannot read {}: {error}", args.path));
                }
            };
            let matches = text.matches(&args.old_string).count();
            match (matches, args.replace_all) {
                (0, _) => ToolOutput::error(format!("old_string not found in {}", args.path)),
                (count, false) if count > 1 => ToolOutput::error(format!(
                    "old_string matches {count} times in {}; disambiguate or set replace_all",
                    args.path
                )),
                (count, all) => {
                    let updated = if all {
                        text.replace(&args.old_string, &args.new_string)
                    } else {
                        text.replacen(&args.old_string, &args.new_string, 1)
                    };
                    match std::fs::write(&args.path, updated) {
                        Ok(()) => ToolOutput::ok(format!(
                            "replaced {count} occurrence(s) in {}",
                            args.path
                        )),
                        Err(error) => {
                            ToolOutput::error(format!("cannot write {}: {error}", args.path))
                        }
                    }
                }
            }
        })
    }
}

// --------------------------------------------------------------------- glob

struct Glob;

#[derive(Deserialize)]
struct GlobArgs {
    pattern: String,
    path: Option<String>,
}

impl Tool for Glob {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "List files matching a glob pattern (supports `**`). Returns at most 100 paths."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "minLength": 1},
                "path": {"type": "string"},
            },
            "required": ["pattern"],
            "additionalProperties": false,
        })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, arguments: Value) -> CallFuture<'a> {
        boxed(async move {
            let args: GlobArgs = match parse_args(arguments) {
                Ok(args) => args,
                Err(output) => return output,
            };
            let pattern = match &args.path {
                Some(root) => format!("{}/{}", root.trim_end_matches('/'), args.pattern),
                None => args.pattern.clone(),
            };
            let paths = match glob::glob(&pattern) {
                Ok(paths) => paths,
                Err(error) => return ToolOutput::error(format!("invalid pattern: {error}")),
            };
            let mut found: Vec<PathBuf> = paths
                .filter_map(|entry| entry.ok())
                .filter(|path| path.is_file())
                .take(GLOB_CAP + 1)
                .collect();
            let truncated = found.len() > GLOB_CAP;
            found.truncate(GLOB_CAP);
            found.sort();
            let mut out = found
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            if truncated {
                out.push_str(&format!("\n[truncated at {GLOB_CAP} matches]"));
            }
            if out.is_empty() {
                out = "no matches".to_string();
            }
            ToolOutput::ok(out)
        })
    }
}

// --------------------------------------------------------------------- grep

struct Grep;

#[derive(Deserialize)]
struct GrepArgs {
    pattern: String,
    path: Option<String>,
    include: Option<String>,
    max_results: Option<usize>,
}

impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with a Rust regex under a path (default `.`), optionally filtered by an `include` glob such as `*.rs`. Skips binary files and `.git`."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "minLength": 1},
                "path": {"type": "string"},
                "include": {"type": "string"},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 500},
            },
            "required": ["pattern"],
            "additionalProperties": false,
        })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, arguments: Value) -> CallFuture<'a> {
        boxed(async move {
            let args: GrepArgs = match parse_args(arguments) {
                Ok(args) => args,
                Err(output) => return output,
            };
            let regex = match regex::Regex::new(&args.pattern) {
                Ok(regex) => regex,
                Err(error) => return ToolOutput::error(format!("invalid regex: {error}")),
            };
            let include = match &args.include {
                Some(pattern) => match glob::Pattern::new(pattern) {
                    Ok(pattern) => Some(pattern),
                    Err(error) => {
                        return ToolOutput::error(format!("invalid include glob: {error}"));
                    }
                },
                None => None,
            };
            let root = args.path.unwrap_or_else(|| ".".to_string());
            let mut files: Vec<PathBuf> = Vec::new();
            let root_path = Path::new(&root);
            if root_path.is_file() {
                files.push(root_path.to_path_buf());
            } else {
                let pattern = format!("{}/**/*", root.trim_end_matches('/'));
                match glob::glob(&pattern) {
                    Ok(paths) => {
                        files.extend(paths.filter_map(|entry| entry.ok()).filter(|path| {
                            path.is_file()
                                && !path.components().any(|part| part.as_os_str() == ".git")
                                && include
                                    .as_ref()
                                    .is_none_or(|include| include.matches_path(path))
                        }));
                    }
                    Err(error) => {
                        return ToolOutput::error(format!("invalid search path: {error}"));
                    }
                }
            }
            files.sort();
            let cap = args.max_results.unwrap_or(GREP_DEFAULT_CAP);
            let mut hits = Vec::new();
            'files: for file in files {
                let Ok(bytes) = std::fs::read(&file) else {
                    continue;
                };
                if bytes.iter().take(8192).any(|byte| *byte == 0) {
                    continue;
                }
                let Ok(text) = String::from_utf8(bytes) else {
                    continue;
                };
                for (number, line) in text.lines().enumerate() {
                    if regex.is_match(line) {
                        hits.push(format!("{}:{}: {}", file.display(), number + 1, line));
                        if hits.len() >= cap {
                            break 'files;
                        }
                    }
                }
            }
            if hits.is_empty() {
                ToolOutput::ok("no matches".to_string())
            } else {
                ToolOutput::ok(hits.join("\n"))
            }
        })
    }
}

// --------------------------------------------------------------------- bash

struct Bash;

#[derive(Deserialize)]
struct BashArgs {
    command: String,
    timeout_secs: Option<u64>,
}

impl Tool for Bash {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run a shell command via `bash -c` with a timeout (default 120s, max 600s). Combined stdout+stderr is capped; the exit status is reported."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "minLength": 1},
                "timeout_secs": {"type": "integer", "minimum": 1, "maximum": BASH_MAX_TIMEOUT},
            },
            "required": ["command"],
            "additionalProperties": false,
        })
    }

    fn call<'a>(&'a self, arguments: Value) -> CallFuture<'a> {
        boxed(async move {
            let args: BashArgs = match parse_args(arguments) {
                Ok(args) => args,
                Err(output) => return output,
            };
            let timeout = args
                .timeout_secs
                .unwrap_or(BASH_DEFAULT_TIMEOUT)
                .clamp(1, BASH_MAX_TIMEOUT);
            let spawned = tokio::process::Command::new("bash")
                .arg("-c")
                .arg(&args.command)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn();
            let child = match spawned {
                Ok(child) => child,
                Err(error) => return ToolOutput::error(format!("cannot spawn bash: {error}")),
            };
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                child.wait_with_output(),
            )
            .await;
            match result {
                Err(_) => {
                    ToolOutput::error(format!("command timed out after {timeout}s and was killed"))
                }
                Ok(Err(error)) => ToolOutput::error(format!("command failed to run: {error}")),
                Ok(Ok(output)) => {
                    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !stderr.trim().is_empty() {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str("[stderr]\n");
                        text.push_str(&stderr);
                    }
                    let truncated = text.len() > BASH_OUTPUT_CAP;
                    if truncated {
                        text.truncate(BASH_OUTPUT_CAP);
                        text.push_str("\n[output truncated]");
                    }
                    if !output.status.success() {
                        match output.status.code() {
                            Some(code) => text.push_str(&format!("\n[exit status: {code}]")),
                            None => text.push_str("\n[killed by signal]"),
                        }
                    }
                    ToolOutput {
                        text,
                        image: None,
                        is_error: !output.status.success(),
                    }
                }
            }
        })
    }
}

// --------------------------------------------------------------- read_image

struct ReadImage;

#[derive(Deserialize)]
struct ImageArgs {
    path: String,
}

impl Tool for ReadImage {
    fn name(&self) -> &str {
        "read_image"
    }

    fn description(&self) -> &str {
        "Load a PNG/JPEG/GIF/WebP image from disk as model-visible image content (max 20 MiB)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1},
            },
            "required": ["path"],
            "additionalProperties": false,
        })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, arguments: Value) -> CallFuture<'a> {
        boxed(async move {
            let args: ImageArgs = match parse_args(arguments) {
                Ok(args) => args,
                Err(output) => return output,
            };
            let media_type = match Path::new(&args.path)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("png") => "image/png",
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                _ => {
                    return ToolOutput::error(format!(
                        "unsupported image type for {}; expected png/jpg/jpeg/gif/webp",
                        args.path
                    ));
                }
            };
            let bytes = match std::fs::read(&args.path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return ToolOutput::error(format!("cannot read {}: {error}", args.path));
                }
            };
            if bytes.len() as u64 > IMAGE_BYTE_CAP {
                return ToolOutput::error(format!(
                    "image {} exceeds the {} MiB limit",
                    args.path,
                    IMAGE_BYTE_CAP / 1024 / 1024
                ));
            }
            let size = bytes.len();
            let image = ImageData {
                media_type: media_type.to_string(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            };
            ToolOutput::image(
                format!("image {} ({media_type}, {size} bytes)", args.path),
                image,
            )
        })
    }
}

// -------------------------------------------------------------- skill_read

struct SkillRead {
    skills: Vec<Skill>,
}

impl SkillRead {
    fn new(skills: Vec<Skill>) -> Self {
        Self { skills }
    }
}

#[derive(Deserialize)]
struct SkillArgs {
    name: String,
}

impl Tool for SkillRead {
    fn name(&self) -> &str {
        "skill_read"
    }

    fn description(&self) -> &str {
        "Load the full instructions of one skill by name. Read a skill before following its guidance."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "minLength": 1},
            },
            "required": ["name"],
            "additionalProperties": false,
        })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, arguments: Value) -> CallFuture<'a> {
        boxed(async move {
            let args: SkillArgs = match parse_args(arguments) {
                Ok(args) => args,
                Err(output) => return output,
            };
            match self.skills.iter().find(|skill| skill.name == args.name) {
                Some(skill) => ToolOutput::ok(skill.body.clone()),
                None => {
                    let available = self
                        .skills
                        .iter()
                        .map(|skill| skill.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    ToolOutput::error(format!(
                        "no skill named {:?}; available: {}",
                        args.name, available
                    ))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[tokio::test]
    async fn write_then_read_with_offset_and_limit() {
        let dir = tempdir();
        let path = dir.path().join("nested").join("note.txt");
        let path = path.to_string_lossy().into_owned();
        let write = WriteFile;
        let out = write
            .call(json!({"path": path, "content": "one\ntwo\nthree\n"}))
            .await;
        assert!(!out.is_error, "{}", out.text);

        let read = ReadFile;
        let out = read
            .call(json!({"path": path, "offset": 2, "limit": 1}))
            .await;
        assert!(out.text.starts_with("two"), "{}", out.text);
        assert!(out.text.contains("truncated"), "{}", out.text);
    }

    #[tokio::test]
    async fn edit_requires_unique_match_unless_replace_all() {
        let dir = tempdir();
        let path = dir.path().join("dup.txt");
        std::fs::write(&path, "aa aa aa").expect("write");
        let edit = EditFile;

        let missing = edit
            .call(json!({"path": path.to_string_lossy(), "old_string": "zz", "new_string": "bb"}))
            .await;
        assert!(missing.is_error);
        assert!(missing.text.contains("not found"));

        let ambiguous = edit
            .call(json!({"path": path.to_string_lossy(), "old_string": "aa", "new_string": "bb"}))
            .await;
        assert!(ambiguous.is_error);
        assert!(ambiguous.text.contains("3 times"));

        let all = edit
            .call(json!({"path": path.to_string_lossy(), "old_string": "aa", "new_string": "bb", "replace_all": true}))
            .await;
        assert!(!all.is_error, "{}", all.text);
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "bb bb bb");
    }

    #[tokio::test]
    async fn glob_and_grep_find_files_and_lines() {
        let dir = tempdir();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").expect("a");
        std::fs::write(dir.path().join("b.txt"), "fn main() {}\n").expect("b");
        std::fs::create_dir(dir.path().join(".git")).expect("git dir");
        std::fs::write(dir.path().join(".git").join("c.rs"), "fn main() {}\n").expect("c");

        let glob = Glob;
        let out = glob
            .call(json!({"pattern": "*.rs", "path": dir.path().to_string_lossy()}))
            .await;
        assert!(out.text.contains("a.rs"), "{}", out.text);
        assert!(!out.text.contains("b.txt"));

        let grep = Grep;
        let out = grep
            .call(json!({"pattern": "fn main", "path": dir.path().to_string_lossy(), "include": "*.rs"}))
            .await;
        assert!(out.text.contains("a.rs:1:"), "{}", out.text);
        assert!(!out.text.contains("b.txt"));
        assert!(!out.text.contains(".git"), "{}", out.text);
    }

    #[tokio::test]
    async fn bash_captures_output_status_and_timeout() {
        let bash = Bash;
        let out = bash
            .call(json!({"command": "echo hi && echo oops >&2"}))
            .await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("hi"));
        assert!(out.text.contains("[stderr]"));

        let out = bash.call(json!({"command": "exit 3"})).await;
        assert!(out.is_error);
        assert!(out.text.contains("[exit status: 3]"));

        let out = bash
            .call(json!({"command": "sleep 5", "timeout_secs": 1}))
            .await;
        assert!(out.is_error);
        assert!(out.text.contains("timed out"));
    }

    #[tokio::test]
    async fn read_image_loads_png_bytes_as_base64() {
        let dir = tempdir();
        let path = dir.path().join("tiny.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("png");
        let read = ReadImage;
        let out = read.call(json!({"path": path.to_string_lossy()})).await;
        assert!(!out.is_error, "{}", out.text);
        let image = out.image.expect("image");
        assert_eq!(image.media_type, "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .expect("base64"),
            b"\x89PNG\r\n\x1a\n"
        );

        let bad = read.call(json!({"path": "x.bin"})).await;
        assert!(bad.is_error);
    }

    #[tokio::test]
    async fn skill_read_returns_body_or_lists_available() {
        let skills = vec![Skill {
            name: "aegis-desktop-interaction".into(),
            description: "interaction domain operations".into(),
            body: "# Body".into(),
            path: PathBuf::from("/tmp/SKILL.md"),
        }];
        let tool = SkillRead::new(skills);
        let out = tool
            .call(json!({"name": "aegis-desktop-interaction"}))
            .await;
        assert_eq!(out.text, "# Body");
        let missing = tool.call(json!({"name": "nope"})).await;
        assert!(missing.is_error);
        assert!(missing.text.contains("aegis-desktop-interaction"));
    }
}
