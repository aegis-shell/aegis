//! Local account identity and locale-aware clock strings.

use std::ffi::{CStr, c_char};

#[derive(Debug, Clone)]
pub struct Identity {
    pub username: String,
    pub display_name: String,
    pub initials: String,
}

impl Identity {
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
            username,
            display_name,
            initials: if initials.is_empty() {
                "A".into()
            } else {
                initials
            },
        })
    }
}

pub fn clock_strings() -> (String, String) {
    let mut timestamp = 0;
    unsafe {
        libc::time(&mut timestamp);
    }
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    let local = unsafe {
        if libc::localtime_r(&timestamp, local.as_mut_ptr()).is_null() {
            return ("--:--".into(), String::new());
        }
        local.assume_init()
    };
    (strftime(&local, c"%H:%M"), strftime(&local, c"%A, %B %e"))
}

fn strftime(time: &libc::tm, format: &CStr) -> String {
    let mut output = [0 as c_char; 128];
    let len = unsafe { libc::strftime(output.as_mut_ptr(), output.len(), format.as_ptr(), time) };
    if len == 0 {
        String::new()
    } else {
        unsafe { CStr::from_ptr(output.as_ptr()) }
            .to_string_lossy()
            .trim()
            .to_owned()
    }
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
