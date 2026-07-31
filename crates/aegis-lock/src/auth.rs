//! PAM authentication isolated from the Wayland/render thread.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use aegis_lock::{AuthResult, Secret};
use zeroize::Zeroize;

const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_USER_UNKNOWN: c_int = 10;
const PAM_MAXTRIES: c_int = 11;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

static AUTH_ACTIVE: AtomicBool = AtomicBool::new(false);

struct AuthActiveGuard;

impl Drop for AuthActiveGuard {
    fn drop(&mut self) {
        AUTH_ACTIVE.store(false, Ordering::Release);
    }
}

#[repr(C)]
struct PamHandle {
    _private: [u8; 0],
}

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

#[repr(C)]
struct PamConv {
    conv: Option<
        unsafe extern "C" fn(
            c_int,
            *mut *const PamMessage,
            *mut *mut PamResponse,
            *mut c_void,
        ) -> c_int,
    >,
    appdata_ptr: *mut c_void,
}

unsafe extern "C" {
    fn pam_start(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *const PamConv,
        pamh: *mut *mut PamHandle,
    ) -> c_int;
    fn pam_end(pamh: *mut PamHandle, pam_status: c_int) -> c_int;
    fn pam_authenticate(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_acct_mgmt(pamh: *mut PamHandle, flags: c_int) -> c_int;
}

struct ConversationData {
    username: CString,
    password: Vec<u8>,
    /// Set when PAM actually requested the password via the conversation.
    /// If `pam_authenticate` fails without ever prompting, the service
    /// profile is misconfigured (commonly: missing `/etc/pam.d/aegis-lock`,
    /// which falls through to a deny-all `other`), and "wrong password" is a
    /// misleading message — surface that instead of looping the user.
    prompted: bool,
}

pub fn authenticate_async(username: String, secret: Secret, results: Sender<AuthResult>) {
    if AUTH_ACTIVE.swap(true, Ordering::AcqRel) {
        let _ = results.send(AuthResult::Unavailable {
            message: localized(
                "Authentication is still running · Please wait",
                "认证仍在进行 · 请稍候",
            ),
        });
        return;
    }
    let spawn_error_results = results.clone();
    let watchdog = std::thread::Builder::new()
        .name("aegis-lock-auth-watchdog".into())
        .spawn(move || {
            let (pam_tx, pam_rx) = std::sync::mpsc::sync_channel(1);
            let worker = std::thread::Builder::new()
                .name("aegis-lock-pam".into())
                .spawn(move || {
                    let _active = AuthActiveGuard;
                    let result = authenticate(&username, secret);
                    let _ = pam_tx.send(result);
                });
            if worker.is_err() {
                AUTH_ACTIVE.store(false, Ordering::Release);
                let _ = results.send(AuthResult::Unavailable {
                    message: localized("Authentication service unavailable", "认证服务不可用"),
                });
                return;
            }
            let result = match pam_rx.recv_timeout(AUTH_TIMEOUT) {
                Ok(result) => result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => AuthResult::Unavailable {
                    message: localized(
                        "Authentication timed out · Please try again",
                        "认证超时 · 请重试",
                    ),
                },
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => AuthResult::Unavailable {
                    message: localized("Authentication service unavailable", "认证服务不可用"),
                },
            };
            let _ = results.send(result);
        });
    if watchdog.is_err() {
        AUTH_ACTIVE.store(false, Ordering::Release);
        let _ = spawn_error_results.send(AuthResult::Unavailable {
            message: localized("Authentication service unavailable", "认证服务不可用"),
        });
    }
}

fn authenticate(username: &str, secret: Secret) -> AuthResult {
    let Ok(username) = CString::new(username) else {
        return AuthResult::Unavailable {
            message: localized("Authentication service unavailable", "认证服务不可用"),
        };
    };
    let Some(password) = secret.into_nul_terminated() else {
        return AuthResult::Rejected {
            message: localized("Incorrect password", "密码错误"),
        };
    };
    let mut data = ConversationData {
        username,
        password,
        prompted: false,
    };
    let conv = PamConv {
        conv: Some(conversation),
        appdata_ptr: ptr::from_mut(&mut data).cast(),
    };
    let mut handle = ptr::null_mut();
    let service = c"aegis-lock";
    // SAFETY: all pointers remain live until pam_end; the callback allocates
    // each response with libc so PAM may release it using free(3).
    let mut status =
        unsafe { pam_start(service.as_ptr(), data.username.as_ptr(), &conv, &mut handle) };
    if status == PAM_SUCCESS {
        status = unsafe { pam_authenticate(handle, 0) };
    }
    if status == PAM_SUCCESS {
        status = unsafe { pam_acct_mgmt(handle, 0) };
    }
    if !handle.is_null() {
        unsafe {
            pam_end(handle, status);
        }
    }
    data.password.zeroize();

    match status {
        PAM_SUCCESS => AuthResult::Accepted,
        // PAM rejected only after asking for the password: a genuine
        // wrong-password attempt.
        PAM_AUTH_ERR | PAM_USER_UNKNOWN | PAM_MAXTRIES if data.prompted => AuthResult::Rejected {
            message: localized(
                "Incorrect password · Please wait before trying again",
                "密码错误 · 请稍后再试",
            ),
        },
        // PAM rejected without ever prompting: the `aegis-lock` service
        // profile is absent or deny-all (the common case when the distro
        // package was not installed). The password is almost certainly fine,
        // so looping "wrong password" is misleading — point at the fix.
        PAM_AUTH_ERR | PAM_USER_UNKNOWN | PAM_MAXTRIES => AuthResult::Unavailable {
            message: localized(
                "Authentication misconfigured · Install the aegis-lock PAM profile",
                "认证配置异常 · 请安装 aegis-lock PAM 配置",
            ),
        },
        _ => AuthResult::Unavailable {
            message: localized("Authentication service unavailable", "认证服务不可用"),
        },
    }
}

unsafe extern "C" fn conversation(
    count: c_int,
    messages: *mut *const PamMessage,
    responses: *mut *mut PamResponse,
    appdata: *mut c_void,
) -> c_int {
    if count <= 0 || messages.is_null() || responses.is_null() || appdata.is_null() {
        return PAM_CONV_ERR;
    }
    let Ok(count) = usize::try_from(count) else {
        return PAM_CONV_ERR;
    };
    let data = unsafe { &mut *(appdata.cast::<ConversationData>()) };
    let allocation =
        unsafe { libc::calloc(count, std::mem::size_of::<PamResponse>()) }.cast::<PamResponse>();
    if allocation.is_null() {
        return PAM_BUF_ERR;
    }

    for index in 0..count {
        let message = unsafe { *messages.add(index) };
        if message.is_null() {
            unsafe { free_responses(allocation, count) };
            return PAM_CONV_ERR;
        }
        let source = match unsafe { (*message).msg_style } {
            // A real prompt means the configured stack is asking for a
            // credential, so a later rejection is a genuine wrong-password.
            PAM_PROMPT_ECHO_OFF => {
                data.prompted = true;
                data.password.as_ptr().cast::<c_char>()
            }
            PAM_PROMPT_ECHO_ON => {
                data.prompted = true;
                data.username.as_ptr()
            }
            PAM_ERROR_MSG | PAM_TEXT_INFO => ptr::null(),
            _ => {
                unsafe { free_responses(allocation, count) };
                return PAM_CONV_ERR;
            }
        };
        if !source.is_null() {
            let duplicate = unsafe { libc::strdup(source) };
            if duplicate.is_null() {
                unsafe { free_responses(allocation, count) };
                return PAM_BUF_ERR;
            }
            unsafe {
                (*allocation.add(index)).resp = duplicate;
            }
        }
    }
    unsafe {
        *responses = allocation;
    }
    PAM_SUCCESS
}

unsafe fn free_responses(responses: *mut PamResponse, count: usize) {
    for index in 0..count {
        let response = unsafe { (*responses.add(index)).resp };
        if !response.is_null() {
            let length = unsafe { CStr::from_ptr(response) }
                .to_bytes_with_nul()
                .len();
            unsafe {
                ptr::write_bytes(response.cast::<u8>(), 0, length);
                libc::free(response.cast());
            }
        }
    }
    unsafe {
        libc::free(responses.cast());
    }
}

fn localized(en: &str, zh: &str) -> String {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if locale.starts_with("zh") {
        zh.to_owned()
    } else {
        en.to_owned()
    }
}
