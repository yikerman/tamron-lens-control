#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
#![warn(missing_docs)]

//! Stateless, semantic control of compatible Tamron lenses on Linux.

#[cfg(not(target_os = "linux"))]
compile_error!("tamron-lens-control supports Linux only");

mod device;
mod error;
mod lens;
mod protocol;
mod snapshot;

pub use device::{DeviceInfo, discover_devices, select_device};
pub use error::{Error, Result};
pub use lens::{
    AfLimit, ButtonFunction, ButtonSettings, ButtonSlot, Capabilities, ConnectionState,
    FocusRingDirection, FocusRingFunction, FocusRingResponse, Lens, LensClass, LensInfo,
    LensSettings, LimitPosition, Mount, RingSetting, SettingChange, SwitchMode,
};
pub use snapshot::SettingsSnapshot;
