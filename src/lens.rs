use std::fmt;

use crate::{DeviceInfo, Error, Result, SettingsSnapshot, protocol};

const DESCRIPTOR_REGION: u8 = 0;
const SETTINGS_REGION: u8 = 1;

/// Connection state returned by the lens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    /// Lens is connected directly to the computer.
    Standalone,
    /// Lens reports that a camera body is attached.
    CameraAttached,
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Standalone => "standalone",
            Self::CameraAttached => "camera-attached",
        })
    }
}

/// Lens mount reported by the descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mount {
    /// Sony E mount.
    SonyE,
    /// Canon RF mount.
    CanonRf,
    /// Nikon Z mount.
    NikonZ,
}

impl fmt::Display for Mount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SonyE => "Sony E",
            Self::CanonRf => "Canon RF",
            Self::NikonZ => "Nikon Z",
        })
    }
}

/// Mechanical lens classification derived from descriptor flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LensClass {
    /// Single-focal-length lens.
    Prime,
    /// Zoom lens whose zoom ring is behind the other ring.
    BackRingZoom,
    /// Zoom lens whose zoom ring is in front.
    FrontRingZoom,
}

impl fmt::Display for LensClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Prime => "prime",
            Self::BackRingZoom => "back-ring zoom",
            Self::FrontRingZoom => "front-ring zoom",
        })
    }
}

/// One AF-limit position advertised by a lens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LimitPosition {
    /// Storage index used by AF-limit setters.
    pub index: u8,
    /// Raw descriptor value.
    pub raw_value: u8,
    /// Approximate user-facing distance label.
    pub label: String,
}

/// Descriptor capability fields consumed by the semantic API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capabilities {
    feature_bits: u32,
    switch_mode_bits: u8,
    button_function_bits: u32,
}

impl Capabilities {
    /// Whether a descriptor feature bit is set.
    pub fn supports_feature(&self, bit: u8) -> bool {
        self.feature_bits & (1_u32 << bit) != 0
    }

    /// Whether a custom-switch mode is advertised.
    pub fn supports_switch_mode(&self, mode: SwitchMode) -> bool {
        mode.raw()
            .filter(|value| *value < 8)
            .is_some_and(|value| self.switch_mode_bits & (1_u8 << value) != 0)
    }

    /// Whether a button function is advertised. Clearing an assignment is always allowed.
    pub fn supports_button_function(&self, function: ButtonFunction) -> bool {
        function == ButtonFunction::None
            || function
                .raw()
                .is_some_and(|value| self.button_function_bits & (1_u32 << value) != 0)
    }

    /// Raw feature bits for diagnostic presentation.
    pub fn feature_bits(&self) -> u32 {
        self.feature_bits
    }

    /// Raw custom-switch mode capability bits.
    pub fn switch_mode_bits(&self) -> u8 {
        self.switch_mode_bits
    }

    /// Raw button-function capability bits.
    pub fn button_function_bits(&self) -> u32 {
        self.button_function_bits
    }
}

/// Identity, ranges, and capabilities loaded from descriptor memory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LensInfo {
    /// Current connection state.
    pub connection_state: ConnectionState,
    /// Product name.
    pub product_name: String,
    /// Display model identifier.
    pub model_name: String,
    /// Raw model identifier used by snapshot compatibility checks.
    pub model_id: [u8; 8],
    /// Firmware major byte.
    pub firmware_major: u8,
    /// Firmware minor byte.
    pub firmware_minor: u8,
    /// Lens mount.
    pub mount: Mount,
    /// Lens mechanical classification.
    pub lens_class: LensClass,
    /// Number of physical focus-set buttons used for presentation.
    pub focus_button_count: u8,
    /// Number of visible custom-switch positions.
    pub switch_position_count: u8,
    /// Advertised semantic capabilities.
    pub capabilities: Capabilities,
    /// Supported focus-ring rotation angles in degrees.
    pub focus_angles: Vec<u16>,
    /// Supported aperture-ring rotation angles in degrees.
    pub aperture_angles: Vec<u16>,
    /// Maximum absolute focus calibration value.
    pub calibration_half_range: i8,
    /// Minimum displayed motor speed.
    pub minimum_motor_speed: i8,
    /// Maximum focus/iris duration in tenths of a second.
    pub maximum_duration_tenths: u16,
    /// AF-limit positions in storage order.
    pub limit_positions: Vec<LimitPosition>,
    /// Whether every AF far bound must be infinity.
    pub fixed_infinity: bool,
}

impl LensInfo {
    /// Firmware formatted as two hexadecimal bytes.
    pub fn firmware_version(&self) -> String {
        format!("{:02X}.{:02X}", self.firmware_major, self.firmware_minor)
    }

    fn parse(bytes: &[u8; 256], connection_state: ConnectionState) -> Result<Self> {
        let model_id: [u8; 8] = bytes[16..24].try_into().unwrap();
        let feature_bits = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
        let button_function_bits = u32::from_le_bytes(bytes[48..52].try_into().unwrap());
        let focus_angles = index_range(bytes[7], |index| u16::from(index + 1) * 90)?;
        let aperture_angles = index_range(bytes[11], |index| u16::from(index) * 15 + 45)?;
        let calibration = bytes[8].max(1).min(i8::MAX as u8) as i8;
        let maximum_speed_index = bytes[9].min(9) as i8;
        let duration_leading_digit = (bytes[10] / 10).min(9);
        let maximum_duration_tenths = u16::from(duration_leading_digit) * 100 + 99;

        let mut limit_positions = Vec::new();
        for (index, value) in bytes[64..80].iter().copied().enumerate() {
            if index != 0 && value == 0 {
                break;
            }
            limit_positions.push(LimitPosition {
                index: index as u8,
                raw_value: value,
                label: match value {
                    0 => "near".into(),
                    0xff => "inf".into(),
                    1..=9 => format!("0.{value}m"),
                    _ => format!("{}m", value / 10),
                },
            });
        }
        Ok(Self {
            connection_state,
            product_name: terminated_text(&bytes[96..160]),
            model_name: terminated_text(&model_id),
            model_id,
            firmware_major: bytes[25],
            firmware_minor: bytes[24],
            mount: match bytes[5] {
                1 => Mount::CanonRf,
                2 => Mount::NikonZ,
                _ => Mount::SonyE,
            },
            lens_class: if bytes[0] & 0x02 != 0 {
                LensClass::Prime
            } else if bytes[0] & 0x04 != 0 {
                LensClass::BackRingZoom
            } else {
                LensClass::FrontRingZoom
            },
            focus_button_count: bytes[3],
            switch_position_count: bytes[4].min(3),
            capabilities: Capabilities {
                feature_bits,
                switch_mode_bits: bytes[40],
                button_function_bits,
            },
            focus_angles,
            aperture_angles,
            calibration_half_range: calibration,
            minimum_motor_speed: 2 - maximum_speed_index,
            maximum_duration_tenths,
            limit_positions,
            fixed_infinity: bytes[15] & 1 != 0,
        })
    }
}

fn index_range(value: u8, convert: impl Fn(u8) -> u16) -> Result<Vec<u16>> {
    let minimum = value >> 4;
    let maximum = value & 0x0f;
    if minimum > maximum {
        return Err(Error::InvalidLensData(format!(
            "descriptor index range {minimum}..{maximum} is reversed"
        )));
    }
    Ok((minimum..=maximum).map(convert).collect())
}

fn terminated_text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

macro_rules! byte_enum {
    ($name:ident { $($variant:ident = $value:literal => $text:literal),+ $(,)? }) => {
        #[doc = concat!("Semantic values for ", stringify!($name), ".")]
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum $name {
            $(#[doc = $text] $variant,)+
            /// An unrecognized value read from a newer or different lens.
            Unknown(u8),
        }

        impl $name {
            fn from_raw(value: u8) -> Self {
                match value {
                    $($value => Self::$variant,)+
                    value => Self::Unknown(value),
                }
            }

            fn raw(self) -> Option<u8> {
                match self {
                    $(Self::$variant => Some($value),)+
                    Self::Unknown(_) => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $(Self::$variant => formatter.write_str($text),)+
                    Self::Unknown(value) => write!(formatter, "unknown(0x{value:02X})"),
                }
            }
        }
    };
}

byte_enum!(FocusRingFunction {
    Focus = 0x00 => "focus",
    Aperture = 0x01 => "aperture",
});

byte_enum!(FocusRingDirection {
    Forward = 0x00 => "forward",
    Reverse = 0x01 => "reverse",
    Camera = 0x02 => "camera",
});

byte_enum!(FocusRingResponse {
    Nonlinear = 0x00 => "nonlinear",
    Linear = 0x01 => "linear",
});

byte_enum!(SwitchMode {
    AfLimit = 0x00 => "af-limit",
    AfLimitMf = 0x01 => "af-limit-mf",
    MultiSelect = 0x04 => "multi-select",
});

byte_enum!(ButtonFunction {
    None = 0x00 => "none",
    AfMfHold = 0x01 => "af-mf-hold",
    AfMfPress = 0x02 => "af-mf-press",
    AfLimitHold = 0x03 => "af-limit-hold",
    AfLimitPress = 0x04 => "af-limit-press",
    FocusHold = 0x05 => "focus-hold",
    AfLimitWhilePressed = 0x06 => "af-limit-while-pressed",
    FullRangeWhilePressed = 0x07 => "full-range-while-pressed",
    FocusPreset = 0x08 => "focus-preset",
    AbFocus = 0x09 => "a-b-focus",
    FocusHold2 = 0x0a => "focus-hold-2",
    AstroFine = 0x0b => "astro-fine",
    RingSwitchHold = 0x0c => "ring-switch-hold",
    RingSwitchPress = 0x0d => "ring-switch-press",
    AstroFixedHold = 0x0e => "astro-fixed-hold",
    AstroFixedPress = 0x0f => "astro-fixed-press",
    FocusStopperHold = 0x10 => "focus-stopper-hold",
    FocusStopperPress = 0x11 => "focus-stopper-press",
    VcHold = 0x12 => "vc-hold",
    VcPress = 0x13 => "vc-press",
    FocusTimelapseHold = 0x14 => "focus-timelapse-hold",
    TimedFocusPreset = 0x16 => "timed-focus-preset",
    TimedAbFocus = 0x17 => "timed-a-b-focus",
    TimedIrisPreset = 0x18 => "timed-iris-preset",
    TimedAbIris = 0x19 => "timed-a-b-iris",
    MfResponseHold = 0x1a => "mf-response-hold",
    MfResponsePress = 0x1b => "mf-response-press",
    RingStopperHold = 0x1c => "ring-stopper-hold",
    RingStopperPress = 0x1d => "ring-stopper-press",
});

impl ButtonFunction {
    fn uses_speed(self) -> bool {
        matches!(self, Self::FocusPreset | Self::AbFocus)
    }

    fn uses_duration(self) -> bool {
        self.timing_offsets().is_some()
    }

    fn timing_offsets(self) -> Option<(u8, u8)> {
        match self {
            Self::TimedFocusPreset | Self::TimedAbFocus => Some((32, 48)),
            Self::TimedIrisPreset | Self::TimedAbIris => Some((40, 56)),
            _ => None,
        }
    }

    fn uses_counts(self) -> bool {
        self == Self::FocusTimelapseHold
    }

    fn uses_af_limit(self) -> bool {
        matches!(
            self,
            Self::AfLimitHold
                | Self::AfLimitPress
                | Self::AfLimitWhilePressed
                | Self::FullRangeWhilePressed
        )
    }
}

/// Logical assignment slot used by button settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ButtonSlot {
    /// Focus-set button assignment.
    Focus,
    /// Custom-switch position 1 assignment.
    Custom1,
    /// Custom-switch position 2 assignment.
    Custom2,
    /// Custom-switch position 3 assignment.
    Custom3,
}

impl ButtonSlot {
    /// Zero-based settings-image slot index.
    pub fn index(self) -> usize {
        match self {
            Self::Focus => 0,
            Self::Custom1 => 1,
            Self::Custom2 => 2,
            Self::Custom3 => 3,
        }
    }
}

impl fmt::Display for ButtonSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Focus => "focus",
            Self::Custom1 => "custom-1",
            Self::Custom2 => "custom-2",
            Self::Custom3 => "custom-3",
        })
    }
}

/// One packed near/far autofocus limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AfLimit {
    /// Near descriptor-table index.
    pub near_index: u8,
    /// Far descriptor-table index.
    pub far_index: u8,
}

/// Current settings for one logical button slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ButtonSettings {
    /// Assigned function.
    pub function: ButtonFunction,
    /// Displayed movement speed.
    pub speed: i16,
    /// Actuation duration in tenths of a second.
    pub duration_tenths: u16,
    /// Pre-actuation delay in tenths of a second.
    pub delay_tenths: u16,
    /// Fixed-focus exposure count.
    pub skip_count: u16,
    /// Moving-focus exposure count, normalized to at least one.
    pub move_count: u16,
    /// Per-function autofocus limit.
    pub af_limit: AfLimit,
}

/// Parsed semantic view of the 512-byte settings image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LensSettings {
    /// Current focus-ring assignment.
    pub ring_function: FocusRingFunction,
    /// Current focus-ring direction.
    pub ring_direction: FocusRingDirection,
    /// Current focus-ring response.
    pub ring_response: FocusRingResponse,
    /// Focus-ring angle in degrees.
    pub ring_angle: u16,
    /// Aperture-ring angle in degrees.
    pub aperture_angle: u16,
    /// Manual-focus override sensitivity.
    pub override_sensitivity: i8,
    /// Focus-point calibration.
    pub focus_adjustment: i8,
    /// Current custom-switch mode.
    pub switch_mode: SwitchMode,
    /// Physical custom-switch AF limits for positions 1 through 3.
    pub switch_limits: [AfLimit; 3],
    /// Logical button slot settings.
    pub buttons: [ButtonSettings; 4],
    raw: [u8; 512],
}

impl LensSettings {
    fn parse(raw: [u8; 512], info: &LensInfo) -> Self {
        let limit = |value: u8| decode_limit(value, info.limit_positions.len());
        Self {
            ring_function: FocusRingFunction::from_raw(raw[0]),
            ring_direction: FocusRingDirection::from_raw(raw[1]),
            ring_response: FocusRingResponse::from_raw(raw[2]),
            ring_angle: (u16::from(raw[3]) + 1) * 90,
            aperture_angle: u16::from(raw[5]) * 15 + 45,
            override_sensitivity: raw[9] as i8,
            focus_adjustment: raw[19] as i8,
            switch_mode: SwitchMode::from_raw(raw[64]),
            switch_limits: [limit(raw[16]), limit(raw[17]), limit(raw[18])],
            buttons: std::array::from_fn(|slot| {
                let function = ButtonFunction::from_raw(raw[80 + slot]);
                let (duration_offset, delay_offset) = function.timing_offsets().unwrap_or((32, 48));
                ButtonSettings {
                    function,
                    skip_count: read_u16(&raw, 84 + 2 * slot).min(9999),
                    move_count: read_u16(&raw, 92 + 2 * slot).clamp(1, 9999),
                    speed: 2 - i16::from(raw[96 + slot]),
                    duration_tenths: read_u16(&raw, 256 + usize::from(duration_offset) + 2 * slot),
                    delay_tenths: read_u16(&raw, 256 + usize::from(delay_offset) + 2 * slot),
                    af_limit: limit(if slot == 0 { raw[16] } else { raw[223 + slot] }),
                }
            }),
            raw,
        }
    }

    /// Return the exact lossless settings image.
    pub fn raw_image(&self) -> &[u8; 512] {
        &self.raw
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn decode_limit(value: u8, count: usize) -> AfLimit {
    let last = count.saturating_sub(1).min(15) as u8;
    AfLimit {
        near_index: (value & 0x0f).min(last),
        far_index: (value >> 4).min(last),
    }
}

/// Individual ring setting selectors used by clients that present partial views.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RingSetting {
    /// Ring focus/aperture function.
    Function,
    /// Ring direction.
    Direction,
    /// Linear/nonlinear response.
    Response,
    /// Focus rotation angle.
    Angle,
    /// Aperture rotation angle.
    ApertureAngle,
    /// AF-to-MF override sensitivity.
    OverrideSensitivity,
}

/// One typed semantic mutation applied to a connected lens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingChange {
    /// Change focus-ring function.
    RingFunction(FocusRingFunction),
    /// Change focus-ring direction.
    RingDirection(FocusRingDirection),
    /// Change focus-ring response.
    RingResponse(FocusRingResponse),
    /// Change focus-ring rotation angle in degrees.
    RingAngle(u16),
    /// Change aperture-ring rotation angle in degrees.
    ApertureAngle(u16),
    /// Change manual-focus override sensitivity.
    OverrideSensitivity(i8),
    /// Change one logical slot's assigned function.
    ButtonFunction(ButtonSlot, ButtonFunction),
    /// Change one logical slot's displayed speed.
    ButtonSpeed(ButtonSlot, i8),
    /// Change one logical slot's duration in tenths of a second.
    ButtonDuration(ButtonSlot, u16),
    /// Change one logical slot's pre-delay in tenths of a second.
    ButtonDelay(ButtonSlot, u16),
    /// Change one timelapse slot's skip count.
    ButtonSkipCount(ButtonSlot, u16),
    /// Change one timelapse slot's move count; zero is normalized to one.
    ButtonMoveCount(ButtonSlot, u16),
    /// Change one logical slot's autofocus limit.
    ButtonAfLimit(ButtonSlot, AfLimit),
    /// Change custom-switch mode.
    SwitchMode(SwitchMode),
    /// Change the AF limit for physical switch position 1 through 3.
    SwitchAfLimit(u8, AfLimit),
    /// Change focus-point calibration.
    FocusAdjustment(i8),
}

/// A connected lens with cached descriptor and semantic settings state.
pub struct Lens {
    io: Box<dyn protocol::LensIo>,
    info: LensInfo,
    settings: LensSettings,
}

impl fmt::Debug for Lens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Lens")
            .field("info", &self.info)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl Lens {
    /// Open a serial device, connect, and load descriptor plus settings memory.
    pub fn connect(device: &DeviceInfo) -> Result<Self> {
        Self::initialize(protocol::open(device)?)
    }

    fn initialize(mut io: Box<dyn protocol::LensIo>) -> Result<Self> {
        let connection_state = match io.connect()? {
            1 => ConnectionState::Standalone,
            2 => ConnectionState::CameraAttached,
            3 => {
                let _ = io.disconnect();
                return Err(Error::RecoveryMode);
            }
            value => {
                let _ = io.disconnect();
                return Err(Error::UnsupportedConnectionState(value));
            }
        };
        let (info, settings) = match load_state(io.as_mut(), connection_state) {
            Ok(state) => state,
            Err(error) => {
                let _ = io.disconnect();
                return Err(error);
            }
        };
        Ok(Self { io, info, settings })
    }

    /// Current descriptor-derived information.
    pub fn info(&self) -> &LensInfo {
        &self.info
    }

    /// Current cached settings.
    pub fn settings(&self) -> &LensSettings {
        &self.settings
    }

    /// Whether a logical button slot is present and configurable.
    pub fn supports_slot(&self, slot: ButtonSlot) -> bool {
        match slot {
            ButtonSlot::Focus => self.info.capabilities.supports_feature(0),
            ButtonSlot::Custom1 => {
                self.info.capabilities.supports_feature(1) && self.info.switch_position_count >= 1
            }
            ButtonSlot::Custom2 => {
                self.info.capabilities.supports_feature(1) && self.info.switch_position_count >= 2
            }
            ButtonSlot::Custom3 => {
                self.info.capabilities.supports_feature(1) && self.info.switch_position_count >= 3
            }
        }
    }

    /// Capture the current exact settings image in a versioned snapshot.
    pub fn snapshot(&self) -> SettingsSnapshot {
        SettingsSnapshot::new(
            self.info.model_id,
            self.info.firmware_major,
            self.info.firmware_minor,
            self.settings.raw,
        )
    }

    /// Restore a same-model snapshot and return whether its firmware version differed.
    pub fn restore_snapshot(&mut self, snapshot: &SettingsSnapshot) -> Result<bool> {
        if snapshot.model_id() != &self.info.model_id {
            return Err(Error::SnapshotModelMismatch);
        }
        let firmware_mismatch =
            snapshot.firmware_version() != (self.info.firmware_major, self.info.firmware_minor);
        self.io.restore_settings(snapshot.settings())?;
        self.reload()?;
        Ok(firmware_mismatch)
    }

    /// Restore factory settings and reload descriptor/settings state.
    pub fn factory_reset(&mut self) -> Result<()> {
        self.io.factory_reset()?;
        self.reload()
    }

    /// Apply one validated semantic setting change.
    pub fn apply(&mut self, change: SettingChange) -> Result<()> {
        match change {
            SettingChange::RingFunction(value) => {
                self.require_feature(2, "focus-ring function")?;
                self.write_enum(0, value.raw(), "focus-ring function")
            }
            SettingChange::RingDirection(value) => {
                self.require_feature(3, "focus-ring direction")?;
                self.write_enum(1, value.raw(), "focus-ring direction")
            }
            SettingChange::RingResponse(value) => {
                self.require_feature(4, "focus-ring response")?;
                self.write_enum(2, value.raw(), "focus-ring response")
            }
            SettingChange::RingAngle(value) => {
                self.require_feature(5, "focus-ring rotation angle")?;
                if self.settings.ring_response != FocusRingResponse::Linear {
                    return Err(Error::InapplicableSetting(
                        "ring angle requires linear ring response".into(),
                    ));
                }
                let index = self
                    .info
                    .focus_angles
                    .iter()
                    .position(|candidate| *candidate == value)
                    .ok_or_else(|| {
                        Error::InvalidValue(format!(
                            "ring angle {value} is not in {:?}",
                            self.info.focus_angles
                        ))
                    })?;
                self.commit_byte(
                    0,
                    3,
                    index as u8 + (self.info.focus_angles[0] / 90 - 1) as u8,
                )
            }
            SettingChange::ApertureAngle(value) => {
                self.require_feature(9, "aperture rotation angle")?;
                if self.settings.ring_function != FocusRingFunction::Aperture {
                    return Err(Error::InapplicableSetting(
                        "aperture angle requires the aperture ring function".into(),
                    ));
                }
                if !self.info.aperture_angles.contains(&value) {
                    return Err(Error::InvalidValue(format!(
                        "aperture angle {value} is not in {:?}",
                        self.info.aperture_angles
                    )));
                }
                self.commit_byte(0, 5, ((value - 45) / 15) as u8)
            }
            SettingChange::OverrideSensitivity(value) => {
                self.require_feature(12, "manual-focus override sensitivity")?;
                if !(0..=2).contains(&value) {
                    return Err(Error::InvalidValue(
                        "override sensitivity must be 0, 1, or 2".into(),
                    ));
                }
                self.commit_byte(0, 9, value as u8)
            }
            SettingChange::ButtonFunction(slot, function) => {
                self.require_slot(slot)?;
                if matches!(function, ButtonFunction::Unknown(_)) {
                    return Err(Error::InvalidValue("unknown button function".into()));
                }
                if !self.info.capabilities.supports_button_function(function) {
                    return Err(Error::UnsupportedSetting(format!(
                        "button function {function}"
                    )));
                }
                let mut functions = self.button_functions();
                functions[slot.index()] = function;
                validate_overlap(&functions)?;
                self.commit_byte(0, 80 + slot.index() as u8, function.raw().unwrap())
            }
            SettingChange::ButtonSpeed(slot, value) => {
                self.require_button_use(slot, ButtonFunction::uses_speed, "speed")?;
                validate_overlap(&self.button_functions())?;
                if value < self.info.minimum_motor_speed || value > 2 {
                    return Err(Error::InvalidValue(format!(
                        "speed must be between {} and 2",
                        self.info.minimum_motor_speed
                    )));
                }
                self.commit_byte(0, 96 + slot.index() as u8, (2 - value) as u8)
            }
            SettingChange::ButtonDuration(slot, value) => {
                self.require_button_use(slot, ButtonFunction::uses_duration, "duration")?;
                if value > self.info.maximum_duration_tenths {
                    return Err(Error::InvalidValue(format!(
                        "duration must not exceed {}.{} seconds",
                        self.info.maximum_duration_tenths / 10,
                        self.info.maximum_duration_tenths % 10
                    )));
                }
                let (duration_offset, _) = self.settings.buttons[slot.index()]
                    .function
                    .timing_offsets()
                    .expect("duration applicability requires timing offsets");
                self.commit_word(1, duration_offset + 2 * slot.index() as u8, value)
            }
            SettingChange::ButtonDelay(slot, value) => {
                self.require_feature(10, "pre-actuation delay")?;
                self.require_button_use(slot, ButtonFunction::uses_duration, "delay")?;
                if value > 99 {
                    return Err(Error::InvalidValue(
                        "delay must be between 0.0 and 9.9 seconds".into(),
                    ));
                }
                let (_, delay_offset) = self.settings.buttons[slot.index()]
                    .function
                    .timing_offsets()
                    .expect("delay applicability requires timing offsets");
                self.commit_word(1, delay_offset + 2 * slot.index() as u8, value)
            }
            SettingChange::ButtonSkipCount(slot, value) => {
                self.require_button_use(slot, ButtonFunction::uses_counts, "skip count")?;
                validate_overlap(&self.button_functions())?;
                if value > 9999 {
                    return Err(Error::InvalidValue(
                        "skip count must be between 0 and 9999".into(),
                    ));
                }
                self.write_count(slot, 84, value)
            }
            SettingChange::ButtonMoveCount(slot, value) => {
                self.require_button_use(slot, ButtonFunction::uses_counts, "move count")?;
                validate_overlap(&self.button_functions())?;
                if value > 9999 {
                    return Err(Error::InvalidValue(
                        "move count must be between 0 and 9999".into(),
                    ));
                }
                self.write_count(slot, 92, value.max(1))
            }
            SettingChange::ButtonAfLimit(slot, value) => {
                self.require_button_use(slot, ButtonFunction::uses_af_limit, "AF limit")?;
                self.validate_limit(value)?;
                let offset = if slot == ButtonSlot::Focus {
                    16
                } else {
                    223 + slot.index() as u8
                };
                self.commit_byte(0, offset, pack_limit(value))
            }
            SettingChange::SwitchMode(mode) => {
                self.require_feature(1, "custom-switch assignment")?;
                let value = mode
                    .raw()
                    .ok_or_else(|| Error::InvalidValue("unknown custom-switch mode".into()))?;
                if !self.info.capabilities.supports_switch_mode(mode) {
                    return Err(Error::UnsupportedSetting(format!(
                        "custom-switch mode {mode}"
                    )));
                }
                self.commit_byte(0, 64, value)
            }
            SettingChange::SwitchAfLimit(position, value) => {
                self.require_feature(1, "custom-switch assignment")?;
                if position == 0 || position > self.info.switch_position_count {
                    return Err(Error::InvalidValue(format!(
                        "switch position must be between 1 and {}",
                        self.info.switch_position_count
                    )));
                }
                self.validate_limit(value)?;
                self.commit_byte(0, 15 + position, pack_limit(value))
            }
            SettingChange::FocusAdjustment(value) => {
                self.require_feature(7, "focus-point adjustment")?;
                let range = self.info.calibration_half_range;
                if value < -range || value > range {
                    return Err(Error::InvalidValue(format!(
                        "focus adjustment must be between {} and {}",
                        -range, range
                    )));
                }
                self.commit_byte(0, 19, value as u8)
            }
        }
    }

    /// Send the explicit disconnect notification and close the session.
    pub fn disconnect(mut self) -> Result<()> {
        self.io.disconnect()
    }

    fn reload(&mut self) -> Result<()> {
        let (info, settings) = load_state(self.io.as_mut(), self.info.connection_state)?;
        self.info = info;
        self.settings = settings;
        Ok(())
    }

    fn require_feature(&self, bit: u8, name: &str) -> Result<()> {
        if self.info.capabilities.supports_feature(bit) {
            Ok(())
        } else {
            Err(Error::UnsupportedSetting(name.into()))
        }
    }

    fn require_slot(&self, slot: ButtonSlot) -> Result<()> {
        if self.supports_slot(slot) {
            Ok(())
        } else {
            Err(Error::UnsupportedSetting(format!("button slot {slot}")))
        }
    }

    fn require_button_use(
        &self,
        slot: ButtonSlot,
        predicate: fn(ButtonFunction) -> bool,
        setting: &str,
    ) -> Result<()> {
        self.require_slot(slot)?;
        let function = self.settings.buttons[slot.index()].function;
        if predicate(function) {
            Ok(())
        } else {
            Err(Error::InapplicableSetting(format!(
                "{setting} is not used by {slot} function {function}"
            )))
        }
    }

    fn validate_limit(&self, value: AfLimit) -> Result<()> {
        let count = self.info.limit_positions.len() as u8;
        if count < 2 {
            return Err(Error::UnsupportedSetting(
                "lens advertises fewer than two AF-limit positions".into(),
            ));
        }
        if value.near_index >= count.saturating_sub(1)
            || value.far_index == 0
            || value.far_index >= count
            || value.near_index >= value.far_index
        {
            return Err(Error::InvalidValue(format!(
                "AF limit requires near 0..{}, far 1..{}, and near < far",
                count.saturating_sub(2),
                count.saturating_sub(1)
            )));
        }
        if self.info.fixed_infinity && value.far_index != count - 1 {
            return Err(Error::InvalidValue(format!(
                "this lens fixes far to infinity index {}",
                count - 1
            )));
        }
        Ok(())
    }

    fn button_functions(&self) -> [ButtonFunction; 4] {
        std::array::from_fn(|slot| self.settings.buttons[slot].function)
    }

    fn write_enum(&mut self, offset: u8, value: Option<u8>, name: &str) -> Result<()> {
        let value = value.ok_or_else(|| Error::InvalidValue(format!("unknown {name}")))?;
        self.commit_byte(0, offset, value)
    }

    fn write_count(&mut self, slot: ButtonSlot, base: u8, value: u16) -> Result<()> {
        self.commit_word(0, base + 2 * slot.index() as u8, value)?;
        self.commit_word(0, 100 + 2 * slot.index() as u8, 0)
    }

    fn commit_byte(&mut self, block: u8, offset: u8, value: u8) -> Result<()> {
        self.io.write_byte(block, offset, value)?;
        self.settings.raw[usize::from(block) * 256 + usize::from(offset)] = value;
        self.reparse_settings();
        Ok(())
    }

    fn commit_word(&mut self, block: u8, offset: u8, value: u16) -> Result<()> {
        self.io.write_word(block, offset, value)?;
        let absolute = usize::from(block) * 256 + usize::from(offset);
        self.settings.raw[absolute..absolute + 2].copy_from_slice(&value.to_le_bytes());
        self.reparse_settings();
        Ok(())
    }

    fn reparse_settings(&mut self) {
        self.settings = LensSettings::parse(self.settings.raw, &self.info);
    }
}

fn load_state(
    io: &mut dyn protocol::LensIo,
    connection_state: ConnectionState,
) -> Result<(LensInfo, LensSettings)> {
    let descriptor = io.read_block(DESCRIPTOR_REGION, 0)?;
    let first = io.read_block(SETTINGS_REGION, 0)?;
    let second = io.read_block(SETTINGS_REGION, 1)?;
    let info = LensInfo::parse(&descriptor, connection_state)?;
    let mut raw = [0_u8; 512];
    raw[..256].copy_from_slice(&first);
    raw[256..].copy_from_slice(&second);
    let settings = LensSettings::parse(raw, &info);
    Ok((info, settings))
}

fn pack_limit(value: AfLimit) -> u8 {
    (value.far_index << 4) | value.near_index
}

fn validate_overlap(functions: &[ButtonFunction; 4]) -> Result<()> {
    if functions[2].uses_counts() && (functions[0].uses_speed() || functions[1].uses_speed()) {
        return Err(Error::OverlappingSettings(
            "custom-2 move count shares bytes with focus/custom-1 speed".into(),
        ));
    }
    if functions[3].uses_counts() && (functions[2].uses_speed() || functions[3].uses_speed()) {
        return Err(Error::OverlappingSettings(
            "custom-3 move count shares bytes with custom-2/custom-3 speed".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    enum Call {
        Connect(u8),
        Read(Box<[u8; 256]>),
        Byte(u8, u8, u8),
        Word(u8, u8, u16),
        Restore(Box<[u8; 512]>),
        Factory,
        Disconnect,
    }

    struct FakeIo(VecDeque<Call>);

    impl protocol::LensIo for FakeIo {
        fn connect(&mut self) -> Result<u8> {
            match self.0.pop_front().unwrap() {
                Call::Connect(value) => Ok(value),
                _ => panic!("unexpected call"),
            }
        }

        fn read_block(&mut self, _region: u8, _block: u8) -> Result<[u8; 256]> {
            match self.0.pop_front().unwrap() {
                Call::Read(value) => Ok(*value),
                _ => panic!("unexpected call"),
            }
        }

        fn write_byte(&mut self, block: u8, offset: u8, value: u8) -> Result<()> {
            match self.0.pop_front().unwrap() {
                Call::Byte(b, o, v) if (b, o, v) == (block, offset, value) => Ok(()),
                _ => panic!("unexpected byte write"),
            }
        }

        fn write_word(&mut self, block: u8, offset: u8, value: u16) -> Result<()> {
            match self.0.pop_front().unwrap() {
                Call::Word(b, o, v) if (b, o, v) == (block, offset, value) => Ok(()),
                _ => panic!("unexpected word write"),
            }
        }

        fn restore_settings(&mut self, settings: &[u8; 512]) -> Result<()> {
            match self.0.pop_front().unwrap() {
                Call::Restore(expected) if expected.as_ref() == settings => Ok(()),
                _ => panic!("unexpected restore"),
            }
        }

        fn factory_reset(&mut self) -> Result<()> {
            match self.0.pop_front().unwrap() {
                Call::Factory => Ok(()),
                _ => panic!("unexpected factory reset"),
            }
        }

        fn disconnect(&mut self) -> Result<()> {
            match self.0.pop_front().unwrap() {
                Call::Disconnect => Ok(()),
                _ => panic!("unexpected disconnect"),
            }
        }
    }

    fn descriptor() -> [u8; 256] {
        let mut value = [0_u8; 256];
        value[0] = 2;
        value[3] = 1;
        value[4] = 3;
        value[5] = 0;
        value[7] = 0x03;
        value[8] = 5;
        value[9] = 9;
        value[10] = 90;
        value[11] = 0x03;
        value[16..20].copy_from_slice(b"A067");
        value[24] = 2;
        value[25] = 1;
        value[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        value[40] = 0x13;
        value[48..52].copy_from_slice(&u32::MAX.to_le_bytes());
        value[64] = 1;
        value[65] = 5;
        value[66] = 10;
        value[67] = 0xff;
        value[96..100].copy_from_slice(b"Lens");
        value
    }

    fn initialized_with(extra: impl IntoIterator<Item = Call>) -> Lens {
        let mut calls = VecDeque::from([
            Call::Connect(1),
            Call::Read(Box::new(descriptor())),
            Call::Read(Box::new([0; 256])),
            Call::Read(Box::new([0; 256])),
        ]);
        calls.extend(extra);
        Lens::initialize(Box::new(FakeIo(calls))).unwrap()
    }

    #[test]
    fn parses_identity_and_ranges() {
        let lens = initialized_with([]);
        assert_eq!(lens.info.model_name, "A067");
        assert_eq!(lens.info.firmware_version(), "01.02");
        assert_eq!(lens.info.focus_angles, [90, 180, 270, 360]);
        assert_eq!(lens.info.limit_positions[3].label, "inf");
    }

    #[test]
    fn applies_typed_ring_write() {
        let mut lens = initialized_with([Call::Byte(0, 1, 1), Call::Disconnect]);
        lens.apply(SettingChange::RingDirection(FocusRingDirection::Reverse))
            .unwrap();
        assert_eq!(lens.settings.ring_direction, FocusRingDirection::Reverse);
        lens.disconnect().unwrap();
    }

    #[test]
    fn count_write_resets_tally() {
        let mut lens = initialized_with([
            Call::Byte(0, 80, 0x14),
            Call::Word(0, 84, 12),
            Call::Word(0, 100, 0),
        ]);
        lens.apply(SettingChange::ButtonFunction(
            ButtonSlot::Focus,
            ButtonFunction::FocusTimelapseHold,
        ))
        .unwrap();
        lens.apply(SettingChange::ButtonSkipCount(ButtonSlot::Focus, 12))
            .unwrap();
    }

    #[test]
    fn timed_focus_and_iris_use_separate_timing_fields() {
        let mut lens = initialized_with([
            Call::Byte(0, 80, 0x16),
            Call::Word(1, 32, 123),
            Call::Word(1, 48, 45),
            Call::Byte(0, 81, 0x18),
            Call::Word(1, 42, 234),
            Call::Word(1, 58, 56),
        ]);

        lens.apply(SettingChange::ButtonFunction(
            ButtonSlot::Focus,
            ButtonFunction::TimedFocusPreset,
        ))
        .unwrap();
        lens.apply(SettingChange::ButtonDuration(ButtonSlot::Focus, 123))
            .unwrap();
        lens.apply(SettingChange::ButtonDelay(ButtonSlot::Focus, 45))
            .unwrap();

        lens.apply(SettingChange::ButtonFunction(
            ButtonSlot::Custom1,
            ButtonFunction::TimedIrisPreset,
        ))
        .unwrap();
        lens.apply(SettingChange::ButtonDuration(ButtonSlot::Custom1, 234))
            .unwrap();
        lens.apply(SettingChange::ButtonDelay(ButtonSlot::Custom1, 56))
            .unwrap();

        assert_eq!(lens.settings.buttons[0].duration_tenths, 123);
        assert_eq!(lens.settings.buttons[0].delay_tenths, 45);
        assert_eq!(lens.settings.buttons[1].duration_tenths, 234);
        assert_eq!(lens.settings.buttons[1].delay_tenths, 56);
    }

    #[test]
    fn rejects_invalid_af_window() {
        let mut lens = initialized_with([]);
        let error = lens
            .apply(SettingChange::SwitchAfLimit(
                1,
                AfLimit {
                    near_index: 2,
                    far_index: 2,
                },
            ))
            .unwrap_err();
        assert!(matches!(error, Error::InvalidValue(_)));
    }

    #[test]
    fn camera_attached_state_still_loads_settings() {
        let calls = VecDeque::from([
            Call::Connect(2),
            Call::Read(Box::new(descriptor())),
            Call::Read(Box::new([0; 256])),
            Call::Read(Box::new([0; 256])),
        ]);
        let lens = Lens::initialize(Box::new(FakeIo(calls))).unwrap();
        assert_eq!(lens.info.connection_state, ConnectionState::CameraAttached);
    }

    #[test]
    fn recovery_state_is_reported_without_reading_memory() {
        let calls = VecDeque::from([Call::Connect(3), Call::Disconnect]);
        assert!(matches!(
            Lens::initialize(Box::new(FakeIo(calls))),
            Err(Error::RecoveryMode)
        ));
    }

    #[test]
    fn rejects_function_assignment_that_activates_overlap() {
        let mut lens = initialized_with([Call::Byte(0, 82, 0x14)]);
        lens.apply(SettingChange::ButtonFunction(
            ButtonSlot::Custom2,
            ButtonFunction::FocusTimelapseHold,
        ))
        .unwrap();
        let error = lens
            .apply(SettingChange::ButtonFunction(
                ButtonSlot::Focus,
                ButtonFunction::FocusPreset,
            ))
            .unwrap_err();
        assert!(matches!(error, Error::OverlappingSettings(_)));
    }

    #[test]
    fn lens_without_af_limit_table_still_connects() {
        let mut descriptor = descriptor();
        descriptor[64..80].fill(0);
        let calls = VecDeque::from([
            Call::Connect(1),
            Call::Read(Box::new(descriptor)),
            Call::Read(Box::new([0; 256])),
            Call::Read(Box::new([0; 256])),
        ]);
        let lens = Lens::initialize(Box::new(FakeIo(calls))).unwrap();
        assert_eq!(lens.info.limit_positions.len(), 1);
    }

    #[test]
    fn parsing_unknown_indices_does_not_overflow() {
        let info = LensInfo::parse(&descriptor(), ConnectionState::Standalone).unwrap();
        let mut raw = [0_u8; 512];
        raw[3] = u8::MAX;
        raw[96] = u8::MAX;
        let settings = LensSettings::parse(raw, &info);
        assert_eq!(settings.ring_angle, 23_040);
        assert_eq!(settings.buttons[0].speed, -253);
    }

    #[test]
    fn public_count_api_rejects_values_above_9999() {
        let mut lens = initialized_with([Call::Byte(0, 80, 0x14)]);
        lens.apply(SettingChange::ButtonFunction(
            ButtonSlot::Focus,
            ButtonFunction::FocusTimelapseHold,
        ))
        .unwrap();
        assert!(matches!(
            lens.apply(SettingChange::ButtonSkipCount(ButtonSlot::Focus, 10_000)),
            Err(Error::InvalidValue(_))
        ));
    }

    #[test]
    fn same_model_restore_reloads_settings() {
        let restored = [7_u8; 512];
        let mut lens = initialized_with([
            Call::Restore(Box::new(restored)),
            Call::Read(Box::new(descriptor())),
            Call::Read(Box::new([7; 256])),
            Call::Read(Box::new([7; 256])),
        ]);
        let snapshot = SettingsSnapshot::new(*b"A067\0\0\0\0", 1, 3, restored);
        assert!(lens.restore_snapshot(&snapshot).unwrap());
        assert_eq!(lens.settings.raw_image(), &restored);
    }

    #[test]
    fn different_model_restore_is_rejected_before_io() {
        let mut lens = initialized_with([]);
        let snapshot = SettingsSnapshot::new(*b"OTHER\0\0\0", 1, 2, [0; 512]);
        assert!(matches!(
            lens.restore_snapshot(&snapshot),
            Err(Error::SnapshotModelMismatch)
        ));
    }

    #[test]
    fn factory_reset_reloads_state() {
        let mut lens = initialized_with([
            Call::Factory,
            Call::Read(Box::new(descriptor())),
            Call::Read(Box::new([0; 256])),
            Call::Read(Box::new([0; 256])),
        ]);
        lens.factory_reset().unwrap();
        assert_eq!(lens.settings.ring_direction, FocusRingDirection::Forward);
    }
}
