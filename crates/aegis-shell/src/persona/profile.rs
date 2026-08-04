//! Local account defaults resolved once for a personalized shell profile.

use std::ffi::{CStr, CString, c_char};

/// Account-backed defaults for the effective user's presentation profile.
///
/// These fields are display content and must not be used as authenticated
/// Actor authority. Callers may replace them with an explicit personalized
/// value before sharing the profile with presentation hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub username: String,
    pub display_name: String,
    pub initials: String,
    pub groups: Vec<String>,
}

impl Profile {
    pub fn current() -> Result<Self, std::io::Error> {
        let uid = unsafe { libc::geteuid() };
        let capacity = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
        let capacity = usize::try_from(capacity)
            .ok()
            .filter(|value| *value >= 1024)
            .unwrap_or(16 * 1024);
        let mut buffer = vec![0u8; capacity];
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status != 0 || result.is_null() {
            return Err(std::io::Error::from_raw_os_error(status.max(libc::ENOENT)));
        }
        let record = unsafe { record.assume_init() };
        let username = c_string(record.pw_name);
        let gecos = c_string(record.pw_gecos);
        let display_name = gecos
            .split(',')
            .next()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&username)
            .to_owned();
        let initials = display_name
            .split_whitespace()
            .filter_map(|part| part.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase();
        Ok(Self {
            groups: groups(&username, record.pw_gid),
            username,
            display_name,
            initials: if initials.is_empty() {
                "U".into()
            } else {
                initials
            },
        })
    }

    pub fn fallback() -> Self {
        Self {
            username: "user".into(),
            display_name: "User".into(),
            initials: "U".into(),
            groups: Vec::new(),
        }
    }
}

fn groups(username: &str, primary_gid: libc::gid_t) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(name) = group_name(primary_gid) {
        names.push(name);
    }
    let Ok(c_user) = CString::new(username) else {
        return names;
    };
    let mut count: libc::c_int = 0;
    unsafe {
        libc::getgrouplist(
            c_user.as_ptr(),
            primary_gid,
            std::ptr::null_mut(),
            &mut count,
        );
    }
    if count <= 0 {
        return names;
    }
    let mut gids = vec![0 as libc::gid_t; count as usize];
    let mut actual = count;
    let status =
        unsafe { libc::getgrouplist(c_user.as_ptr(), primary_gid, gids.as_mut_ptr(), &mut actual) };
    if status < 0 {
        return names;
    }
    for gid in gids.into_iter().take(actual.max(0) as usize) {
        if let Some(name) = group_name(gid)
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    names
}

fn group_name(gid: libc::gid_t) -> Option<String> {
    let mut buffer = vec![0u8; 4096];
    let mut record = std::mem::MaybeUninit::<libc::group>::uninit();
    let mut result = std::ptr::null_mut();
    let status = unsafe {
        libc::getgrgid_r(
            gid,
            record.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return None;
    }
    let name = c_string(unsafe { record.assume_init() }.gr_name);
    (!name.is_empty()).then_some(name)
}

fn c_string(pointer: *const c_char) -> String {
    if pointer.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_has_a_stable_shared_profile() {
        let profile = Profile::current().expect("passwd record for test user");
        assert!(!profile.username.is_empty());
        assert!(!profile.display_name.is_empty());
        assert!(!profile.initials.is_empty());
    }
}
