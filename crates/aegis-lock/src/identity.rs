//! Shared local account identity and locale-aware clock strings.

use std::ffi::{CStr, c_char};

pub use aegis_identity::Identity;

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
