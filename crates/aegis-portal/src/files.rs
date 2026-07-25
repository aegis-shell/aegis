//! Portal capture persistence: cache-directory resolution, atomic PNG
//! writes, and `file://` URI rendering.
//!
//! Portal screenshots are not user photo-library material — the frontend may
//! copy them wherever the application asked — so they live under
//! `$XDG_CACHE_HOME/aegis-portal` (falling back to `$XDG_RUNTIME_DIR/aegis-portal`
//! per the same spec carve-out that portals themselves use). Files are
//! written mode `0600` via create-temp-then-rename so a consumer can never
//! observe a partial PNG.

use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The portal capture cache directory: `$XDG_CACHE_HOME/aegis-portal`, else
/// `$XDG_RUNTIME_DIR/aegis-portal`, else `None` (the request fails with
/// response code 2).
pub(crate) fn cache_dir() -> Option<PathBuf> {
    cache_dir_from(
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("XDG_RUNTIME_DIR"),
    )
}

/// Split out for tests: environment variables are process-global.
fn cache_dir_from(
    cache: Option<std::ffi::OsString>,
    runtime: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    cache
        .filter(|dir| !dir.is_empty())
        .or_else(|| runtime.filter(|dir| !dir.is_empty()))
        .map(|base| PathBuf::from(base).join("aegis-portal"))
}

/// Write `png` as `screenshot-<millis>-<token>.png` under `dir`, atomically,
/// returning the final path. `token` is already sanitized to `[A-Za-z0-9_]`
/// by the option parser, so the filename cannot escape `dir`.
pub(crate) fn write_capture(dir: &Path, token: &str, png: &[u8]) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = format!("screenshot-{millis}-{token}.png");
    let final_path = dir.join(&name);
    let temporary = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(png)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &final_path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map(|()| final_path)
}

/// Render an absolute path as a `file://` URI, percent-encoding every byte
/// outside the RFC 3986 unreserved set plus `/`.
pub(crate) fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' => {
                uri.push(byte as char);
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_prefers_cache_home_and_falls_back_to_runtime_dir() {
        let cache = std::ffi::OsString::from("/cache");
        let runtime = std::ffi::OsString::from("/run/user/1000");
        assert_eq!(
            cache_dir_from(Some(cache.clone()), Some(runtime.clone())),
            Some(PathBuf::from("/cache/aegis-portal"))
        );
        assert_eq!(
            cache_dir_from(None, Some(runtime.clone())),
            Some(PathBuf::from("/run/user/1000/aegis-portal"))
        );
        assert_eq!(
            cache_dir_from(Some("".into()), Some(runtime)),
            Some(PathBuf::from("/run/user/1000/aegis-portal"))
        );
        assert_eq!(cache_dir_from(None, None), None);
    }

    #[test]
    fn write_capture_persists_bytes_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!(
            "aegis-portal-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = write_capture(&dir, "tok1", b"png-bytes").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"png-bytes");
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-tok1.png")
        );
        // No temp file lingers next to the result.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // Mode 0600: portal payloads are screen pixels.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_uri_encodes_only_what_it_must() {
        assert_eq!(file_uri(Path::new("/a/b-c_d.png")), "file:///a/b-c_d.png");
        assert_eq!(
            file_uri(Path::new("/a b/ç.png")),
            "file:///a%20b/%C3%A7.png"
        );
    }
}
