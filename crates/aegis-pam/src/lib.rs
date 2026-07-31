//! `pam_aegis`: cache the login authtok for the aegis secret vault.
//!
//! Installed as `auth optional pam_aegis.so` in the login stack (and in the
//! lock screen's stack), this module writes the just-verified password to
//! `$XDG_RUNTIME_DIR/aegis-pam-token` (fallback `/run/user/<uid>/`) with
//! mode 0600. `aegis-portal` consumes and deletes the token to unlock a
//! password-mode vault without prompting — the wssp-pam pattern. The token
//! is written atomically (temp-then-rename) so a reader never observes a
//! partial file, and the in-memory copy is zeroized afterwards.
//!
//! Failure posture: a module error never blocks authentication
//! (`optional`); at worst the vault stays locked and the user is prompted
//! through compositor chrome instead.

use pamsm::{Pam, PamError, PamFlag, PamLibExt, PamServiceModule, pam_module};
use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use zeroize::Zeroize;

/// The token file name under the user's runtime directory.
pub const TOKEN_NAME: &str = "aegis-pam-token";

struct AegisPam;

impl PamServiceModule for AegisPam {
    fn open_session(_pamh: Pam, _flags: PamFlag, _args: Vec<String>) -> PamError {
        PamError::SUCCESS
    }

    fn authenticate(pamh: Pam, _flags: PamFlag, _args: Vec<String>) -> PamError {
        let user = match pamh.get_cached_user() {
            Ok(Some(user)) => user.to_string_lossy().into_owned(),
            _ => return PamError::USER_UNKNOWN,
        };
        let mut authtok = match pamh.get_cached_authtok() {
            Ok(Some(token)) => token.to_string_lossy().into_owned(),
            _ => return PamError::SUCCESS,
        };
        if authtok.is_empty() {
            return PamError::SUCCESS;
        }

        let c_user = match CString::new(user) {
            Ok(user) => user,
            Err(_) => {
                authtok.zeroize();
                return PamError::USER_UNKNOWN;
            }
        };
        // SAFETY: getpwnam's returned pointer is valid for the process
        // lifetime and only read here.
        let passwd = unsafe { libc::getpwnam(c_user.as_ptr()) };
        if passwd.is_null() {
            authtok.zeroize();
            return PamError::USER_UNKNOWN;
        }
        // SAFETY: the passwd struct outlives these reads.
        let (uid, gid) = unsafe { ((*passwd).pw_uid, (*passwd).pw_gid) };

        let result = write_token(uid, gid, authtok.as_bytes());
        authtok.zeroize();
        result
    }

    fn setcred(_pam: Pam, _flags: PamFlag, _args: Vec<String>) -> PamError {
        PamError::SUCCESS
    }

    fn close_session(_pam: Pam, _flags: PamFlag, _args: Vec<String>) -> PamError {
        PamError::SUCCESS
    }
}

/// The token path: `$XDG_RUNTIME_DIR/aegis-pam-token`, falling back to
/// `/run/user/<uid>/`.
fn token_path(uid: libc::uid_t) -> String {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => {
            format!("{}/{TOKEN_NAME}", dir.to_string_lossy())
        }
        _ => format!("/run/user/{uid}/{TOKEN_NAME}"),
    }
}

/// Write the token atomically, mode 0600, owned by the user.
fn write_token(uid: libc::uid_t, gid: libc::gid_t, secret: &[u8]) -> PamError {
    let path = token_path(uid);
    let temporary = format!("{path}.tmp");

    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| PamError::SESSION_ERR)?;
        file.write_all(secret).map_err(|_| PamError::SESSION_ERR)?;
        file.sync_all().map_err(|_| PamError::SESSION_ERR)?;
        std::fs::rename(&temporary, &path).map_err(|_| PamError::SESSION_ERR)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return error;
    }

    let c_path = match CString::new(path) {
        Ok(path) => path,
        Err(_) => return PamError::SESSION_ERR,
    };
    // SAFETY: c_path is a valid NUL-terminated path just written above.
    if unsafe { libc::chown(c_path.as_ptr(), uid, gid) } != 0 {
        return PamError::SESSION_ERR;
    }
    PamError::SUCCESS
}

pam_module!(AegisPam);
