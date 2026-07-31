//! `org.freedesktop.impl.portal.Lockdown`: sandbox restriction policy.
//!
//! The frontend reads these properties for sandboxed applications to learn
//! which capabilities the session disables (kiosk/MDM style). Aegis has no
//! lockdown policy engine, so every flag reads `false` — nothing is
//! restricted. The properties are served read-only: there is no settings
//! surface that could meaningfully toggle them yet.

/// The served lockdown interface: stateless, all flags permissive.
pub(crate) struct LockdownIface;

#[zbus::interface(name = "org.freedesktop.impl.portal.Lockdown")]
impl LockdownIface {
    #[zbus(property, name = "disable-printing")]
    fn disable_printing(&self) -> bool {
        false
    }

    #[zbus(property, name = "disable-save-to-disk")]
    fn disable_save_to_disk(&self) -> bool {
        false
    }

    #[zbus(property, name = "disable-application-handlers")]
    fn disable_application_handlers(&self) -> bool {
        false
    }

    #[zbus(property, name = "disable-location")]
    fn disable_location(&self) -> bool {
        false
    }

    #[zbus(property, name = "disable-camera")]
    fn disable_camera(&self) -> bool {
        false
    }

    #[zbus(property, name = "disable-microphone")]
    fn disable_microphone(&self) -> bool {
        false
    }

    #[zbus(property, name = "disable-sound-output")]
    fn disable_sound_output(&self) -> bool {
        false
    }
}
