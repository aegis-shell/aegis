//! Keyboard keymap and modifier-state tracking via xkbcommon.
//!
//! The server compiles a default keymap at startup from the standard
//! "evdev/pc104/us" RMLVO (matching what Weston, wlroots, and Mutter all use
//! out of the box). Each time a client binds `wl_keyboard`, the server sends
//! the keymap event with a fresh dup of the shared memfd. Key events from the
//! backend flow through `Keyboard::update_key`, which advances the xkbcommon
//! state and reports the resulting modifier mask tuple so the server can post
//! `wl_keyboard.modifiers`.
//!
//! Linux input-event codes (evdev) and xkb keycodes differ by a constant +8
//! offset; xkbcommon historically used evdev+8 to fit a wider keycode space.

use std::ffi::{CString, c_int, c_void};
use std::os::unix::io::RawFd;

use xkbcommon::xkb;

/// Offset between Linux evdev keycodes (BTN_* / KEY_*) and xkbcommon keycodes.
/// xkb expects `evdev_code + 8` for historical AT-set reasons.
pub const EVDEV_TO_XKB_OFFSET: u32 = 8;

// Linux `memfd_create` flags: MFD_CLOEXEC | MFD_ALLOW_SEALING.
const MEMFD_FLAGS: u32 = 3;
// Linux `fcntl` seal flags applied after writing the keymap.
const F_SEAL_SEAL: i32 = 0x0001;
const F_SEAL_SHRINK: i32 = 0x0002;
const F_SEAL_GROW: i32 = 0x0004;
const F_SEAL_WRITE: i32 = 0x0008;
const F_ADD_SEALS: i32 = 1033;

// `mmap` protections and flags.
const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_SHARED: i32 = 0x01;
const MAP_FAILED: usize = !0usize;

// Minimal raw libc surface. Pulling the whole `libc` crate for these few
// symbols is not worth it; they are stable Linux ABI.
unsafe extern "C" {
    fn memfd_create(name: *const std::os::raw::c_char, flags: u32) -> c_int;
    fn ftruncate(fd: c_int, length: i64) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn dup(fd: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: i32,
        flags: i32,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, length: usize) -> c_int;
}

/// Owned xkbcommon state and the shared keymap fd. Cheap to clone-into-client
/// by `dup`-ing the fd per send; the server keeps the original open.
pub struct Keyboard {
    _context: xkb::Context,
    _keymap: xkb::Keymap,
    state: xkb::State,
    /// Memfd holding the keymap contents. Lives for the server's lifetime;
    /// each client gets a dup of it.
    keymap_fd: RawFd,
    /// Size of the keymap file (bytes including the trailing NUL), to pass to
    /// `wl_keyboard.keymap.size`.
    keymap_size: usize,
}

/// Outcome of advancing the xkbcommon state with one key event: the modifier
/// mask tuple the server posts to `wl_keyboard.modifiers`, plus the keysym
/// and printable character the key produced (for compositor-side text input,
/// e.g. the launcher's search box).
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyOutcome {
    pub depressed: u32,
    pub latched: u32,
    pub locked: u32,
    pub group: u32,
    /// XKB keysym for the key (`XKB_KEY_NoSymbol` if none resolved).
    pub keysym: u32,
    /// Printable character the key produced under the current layout and
    /// modifiers, if any. `None` for control keys and plain modifiers.
    pub utf8: Option<char>,
}

impl Keyboard {
    /// Compile the default keymap and back it with a sealed memfd.
    pub fn new() -> std::io::Result<Keyboard> {
        // `Context::new` and `State::new` are infallible at the xkbcommon
        // API level; `Keymap::new_from_names` can fail if the RMLVO is bad.
        // The hardcoded evdev/pc104/us triple is the standard default weston
        // and wlroots use, so failure here means xkbcommon itself is broken.
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "evdev",
            "pc104",
            "us",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| std::io::Error::other("xkb_keymap_new_from_names returned NULL"))?;
        let state = xkb::State::new(&keymap);

        let keymap_string_raw = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        // Re-add the NUL terminator the safe wrapper strips; the Wayland
        // contract is "the fd contents are exactly `size` bytes of NUL-
        // terminated xkb string".
        let keymap_string =
            CString::new(keymap_string_raw).expect("keymap string contained an interior NUL");
        let keymap_size = keymap_string.as_bytes_with_nul().len();

        let fd = Self::create_keymap_fd(&keymap_string, keymap_size)?;
        // keymap_string is dropped here; its bytes are now only in the memfd.

        Ok(Keyboard {
            _context: context,
            _keymap: keymap,
            state,
            keymap_fd: fd,
            keymap_size,
        })
    }

    /// Size of the keymap file (bytes), to pass to `wl_keyboard.keymap.size`.
    pub fn keymap_size(&self) -> usize {
        self.keymap_size
    }

    /// Dup of the keymap fd, suitable for sending to one client. The caller
    /// (libwayland, via `wl_resource_post_event`) closes the dup after the
    /// event is queued; the original fd stays open for the next client.
    pub fn dup_keymap_fd(&self) -> std::io::Result<RawFd> {
        let fd = unsafe { dup(self.keymap_fd) };
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    }

    /// Advance the xkbcommon state with a key event and report the resulting
    /// modifier mask tuple plus the keysym and printable character the key
    /// produced. The server posts `wl_keyboard.modifiers` unconditionally
    /// after each key event so the client's xkbcommon shadow state always
    /// matches ours; a comparison to suppress redundant posts can be layered
    /// later. The keysym/utf8 fields feed compositor-side text input (the
    /// launcher search) when the compositor — rather than a client — owns the
    /// keyboard (ADR-0022).
    pub fn update_key(&mut self, evdev_code: u32, pressed: bool) -> KeyOutcome {
        let direction = if pressed {
            xkb::KeyDirection::Down
        } else {
            xkb::KeyDirection::Up
        };
        let keycode = xkb::Keycode::new(evdev_code + EVDEV_TO_XKB_OFFSET);
        self.state.update_key(keycode, direction);
        let depressed = self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
        let latched = self.state.serialize_mods(xkb::STATE_MODS_LATCHED);
        let locked = self.state.serialize_mods(xkb::STATE_MODS_LOCKED);
        let group = self.state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE);
        // Read the keysym and produced char from the post-update state so
        // same-batch Shift+letter resolves to the shifted glyph. For modifier
        // keys themselves these resolve to NoSymbol / empty.
        let keysym = self.state.key_get_one_sym(keycode).raw();
        let utf8 = self.state.key_get_utf8(keycode).chars().next();
        KeyOutcome {
            depressed,
            latched,
            locked,
            group,
            keysym,
            utf8,
        }
    }

    fn create_keymap_fd(s: &CString, size: usize) -> std::io::Result<RawFd> {
        let name = c"ass-keymap";
        // Safety: the memfd name is a CStr literal; flags are well-defined.
        let fd = unsafe { memfd_create(name.as_ptr(), MEMFD_FLAGS) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // ftruncate to the exact keymap size; the file is empty after
        // memfd_create so the new bytes are zero-filled, then overwritten.
        if unsafe { ftruncate(fd, size as i64) } < 0 {
            let e = std::io::Error::last_os_error();
            unsafe { close(fd) };
            return Err(e);
        }
        // Map, write, unmap. We use PROT_WRITE for the brief write and tear
        // it down before sealing — sealing requires the write mapping closed.
        let map = unsafe {
            mmap(
                std::ptr::null_mut(),
                size,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if map as usize == MAP_FAILED {
            let e = std::io::Error::last_os_error();
            unsafe { close(fd) };
            return Err(e);
        }
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_bytes_with_nul().as_ptr(), map as *mut u8, size);
            munmap(map, size);
        }
        // Seal the file so a hostile client can't mutate the shared copy.
        let seals = F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE;
        if unsafe { fcntl(fd, F_ADD_SEALS, seals) } < 0 {
            // Sealing failure is non-fatal: clients still get correct
            // contents; we lose the hostile-client hardening.
            log::warn!(
                "[keyboard] memfd sealing failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(fd)
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        unsafe { close(self.keymap_fd) };
    }
}
