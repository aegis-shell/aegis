pub(in crate::runtime) fn atomic_write_capture(path: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let destination = std::path::Path::new(path);
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| format!("capture path {path:?} has no file name"))?
        .to_string_lossy();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), nonce));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("create {}: {error}", temporary.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, destination).map_err(|error| {
            format!(
                "commit capture {} → {}: {error}",
                temporary.display(),
                destination.display()
            )
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Build a standards-compliant `text/uri-list` payload for a screenshot that
/// has already been committed to disk. Canonicalization makes relative
/// screenshot directories unambiguous to paste targets; percent encoding is
/// applied to the raw Unix path bytes so non-UTF-8 paths remain representable.
pub(in crate::runtime) fn screenshot_uri_list(path: &str) -> Result<Vec<u8>, String> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::fs::canonicalize(path)
        .map_err(|error| format!("resolve screenshot URI for {path}: {error}"))?;
    let mut uri = Vec::with_capacity(path.as_os_str().as_bytes().len() * 3 + 10);
    uri.extend_from_slice(b"file://");
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(byte);
        } else {
            uri.extend_from_slice(&[b'%', HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]]);
        }
    }
    uri.extend_from_slice(b"\r\n");
    Ok(uri)
}
