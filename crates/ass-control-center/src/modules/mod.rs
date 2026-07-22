mod display;
mod touchpad;
mod unavailable;

#[cfg(test)]
pub(crate) use display::DISPLAY_MODULE_ID;
pub(crate) use display::DisplayModule;
#[cfg(test)]
pub(crate) use touchpad::TOUCHPAD_MODULE_ID;
pub(crate) use touchpad::TouchpadModule;
pub(crate) use unavailable::UnavailableModule;
