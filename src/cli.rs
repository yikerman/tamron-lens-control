use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::Once,
};

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use log::{LevelFilter, Log, Metadata, Record};
use tamron_lens_control::{
    AfLimit, ButtonFunction, ButtonSlot, Error, FocusRingDirection, FocusRingFunction,
    FocusRingResponse, Lens, Result, RingSetting, SettingChange, SettingsSnapshot, SwitchMode,
    discover_devices, select_device,
};

const LONG_ABOUT: &str = "Adjust settings on a compatible Tamron lens over USB on Linux.

Each command connects to the lens, reads its current settings, carries out the requested action, and disconnects. tlc does not run in the background or keep its own copy of your settings.

Use --device with the serial number or port shown by `tlc devices`. When only one compatible lens is connected, --device is optional.

tlc only offers settings supported by the connected lens. Angles are entered in degrees, movement times in seconds, and AF-limit positions by the numbered list shown in `tlc info`.";

#[derive(Debug, Parser)]
#[command(name = "tlc", version, about, long_about = LONG_ABOUT)]
struct Cli {
    /// Choose which connected lens to use.
    #[arg(
        short = 'd',
        long = "device",
        global = true,
        help_heading = "Device selection",
        value_name = "SERIAL_OR_PORT",
        long_help = "Choose a connected lens by its USB serial number or Linux port, such as /dev/ttyUSB0.

Run `tlc devices` to see the available identifiers. You can leave this option out when only one compatible lens is connected. Some lenses do not report a serial number; use their port name instead."
    )]
    device: Option<String>,

    /// Show protocol activity; use -vv to include raw bytes.
    #[arg(
        short = 'v',
        long = "verbose",
        action = ArgAction::Count,
        help_heading = "Diagnostics",
        long_help = "Show what tlc sends to the lens.

Use -v or --verbose to show each connection, read, write, and disconnect operation. Use -vv to also show every raw TX and RX frame as hexadecimal bytes. Diagnostic output is written to stderr and does not change normal command output."
    )]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show connected Tamron lenses.
    #[command(
        long_about = "Show compatible Tamron lenses currently available to tlc, including the identifier accepted by --device and the Linux port name.

This command does not open or change a lens. If no lens appears, tlc prints the Linux commands that may be needed to enable the USB serial driver."
    )]
    Devices,
    /// Show lens details and current settings.
    #[command(
        long_about = "Show the lens name, model, mount, firmware version, connection state, available button functions, AF-limit positions, and all current settings supported by the lens."
    )]
    Info,
    /// View or change focus ring behavior.
    #[command(
        subcommand,
        long_about = "View or change what the focus ring controls, its rotation direction, linear or nonlinear response, rotation angle, and AF-to-MF override sensitivity. tlc checks each choice against the connected lens before applying it."
    )]
    Ring(RingCommand),
    /// View or change button and Custom Switch assignments.
    #[command(
        subcommand,
        long_about = "View or change what the Focus Set Button and Custom Switch positions do. Movement speed, set time, delay, time-lapse counts, and AF limits are available only when they apply to the currently assigned function."
    )]
    Button(ButtonCommand),
    /// View or change Custom Switch behavior.
    #[command(
        subcommand,
        long_about = "View or change how the Custom Switch positions work and set an AF range for each position. To assign shooting functions to Custom 1, 2, or 3, use `tlc button`."
    )]
    Switch(SwitchCommand),
    /// View or change autofocus calibration.
    #[command(
        subcommand,
        long_about = "Fine-tune autofocus accuracy within the range supported by the lens. Use a positive value to correct front focus or a negative value to correct back focus, then confirm the result with test shots."
    )]
    FocusCalibration(FocusCalibrationCommand),
    /// Back up, restore, or reset lens settings.
    #[command(
        subcommand,
        long_about = "Back up all current lens settings to a file, restore a backup made for the same lens model, or return the lens to its factory settings.

tlc checks that a backup is undamaged and was made for the same lens model. Restoring a backup or resetting the lens asks for confirmation before making changes. Pass --yes to skip that prompt."
    )]
    Settings(SettingsCommand),
}

#[derive(Debug, Subcommand)]
enum RingCommand {
    /// Show one focus ring setting or all available settings.
    Get {
        /// Focus ring setting to show.
        #[arg(
            value_enum,
            long_help = "Show only one focus ring setting. Leave this out to show every focus ring setting available on the connected lens."
        )]
        setting: Option<RingSettingArg>,
    },
    /// Change focus ring behavior.
    #[command(subcommand)]
    Set(RingSetCommand),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RingSettingArg {
    Function,
    Direction,
    Response,
    Angle,
    ApertureAngle,
    OverrideSensitivity,
}

impl From<RingSettingArg> for RingSetting {
    fn from(value: RingSettingArg) -> Self {
        match value {
            RingSettingArg::Function => Self::Function,
            RingSettingArg::Direction => Self::Direction,
            RingSettingArg::Response => Self::Response,
            RingSettingArg::Angle => Self::Angle,
            RingSettingArg::ApertureAngle => Self::ApertureAngle,
            RingSettingArg::OverrideSensitivity => Self::OverrideSensitivity,
        }
    }
}

#[derive(Debug, Subcommand)]
enum RingSetCommand {
    /// Choose what the ring controls.
    Function {
        #[arg(
            value_enum,
            long_help = "Choose whether turning the ring adjusts manual focus or aperture. This command is available only on lenses that let the ring switch between those controls."
        )]
        value: RingFunctionArg,
    },
    /// Choose the ring's rotation direction.
    Direction {
        #[arg(
            value_enum,
            long_help = "Choose forward, reverse, or camera-controlled rotation. Camera-controlled rotation follows the direction selected in the camera when the lens and camera support it."
        )]
        value: RingDirectionArg,
    },
    /// Choose how focus responds when the ring turns.
    Response {
        #[arg(
            value_enum,
            long_help = "Choose nonlinear response, where faster turning moves focus farther, or linear response, where focus movement follows the amount the ring turns."
        )]
        value: RingResponseArg,
    },
    /// Set the rotation used for linear manual focus.
    Angle {
        #[arg(
            value_name = "DEGREES",
            long_help = "Set how far the ring turns from the near end to the far end of the focus range. Enter one of the angles supported by the lens, from 90 to 1080 degrees in 90-degree steps. The ring response must already be linear."
        )]
        value: u16,
    },
    /// Set the ring travel used for aperture control.
    ApertureAngle {
        #[arg(
            value_name = "DEGREES",
            long_help = "Set how far the ring turns across its aperture range. Supported lenses normally offer 45, 60, 75, or 90 degrees. The ring must already be assigned to aperture control."
        )]
        value: u16,
    },
    /// Adjust how easily the ring overrides autofocus.
    OverrideSensitivity {
        #[arg(value_name = "LEVEL", value_parser = clap::value_parser!(i8), long_help = "Choose 0, 1, or 2. A higher value makes the lens less likely to leave autofocus when the focus ring is moved accidentally.")]
        value: i8,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RingFunctionArg {
    Focus,
    Aperture,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RingDirectionArg {
    Forward,
    Reverse,
    Camera,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RingResponseArg {
    Nonlinear,
    Linear,
}

#[derive(Debug, Subcommand)]
enum ButtonCommand {
    /// Show one button assignment or all available assignments.
    Get {
        /// Button or Custom Switch position to show.
        #[arg(
            value_enum,
            long_help = "Choose the Focus Set Button (`focus`) or a Custom Switch position. Leave this out to show every button assignment available on the connected lens."
        )]
        button: Option<ButtonArg>,
    },
    /// Change a button assignment or one of its settings.
    Set(ButtonSetArgs),
}

#[derive(Debug, Args)]
struct ButtonSetArgs {
    /// Button or Custom Switch position to change.
    #[arg(
        value_enum,
        long_help = "Choose the Focus Set Button (`focus`) or Custom Switch position 1, 2, or 3. Only positions present on the connected lens can be changed."
    )]
    button: ButtonArg,
    #[command(subcommand)]
    setting: ButtonSetCommand,
}

#[derive(Debug, Subcommand)]
enum ButtonSetCommand {
    /// Choose what this button does.
    Function {
        #[arg(
            value_enum,
            long_help = "Choose the shooting function assigned to this button or Custom Switch position.

Names ending in `-press` respond to a short press; names ending in `-hold` require a one-second hold. Functions beginning with `timed-` use the duration and optional delay settings. Use `none` to clear the assignment. Choices not supported by the connected lens are rejected."
        )]
        value: ButtonFunctionArg,
    },
    /// Set focus movement speed.
    Speed {
        #[arg(
            value_name = "SPEED",
            allow_hyphen_values = true,
            long_help = "Choose how quickly focus moves for Focus Preset or A-B Focus. The available range runs from the lens's slowest setting up to 2, the fastest setting.

Some lens settings share storage internally. tlc refuses the change if it would alter an active Focus Time Lapse setting."
        )]
        value: i8,
    },
    /// Set how long a timed focus or aperture move takes.
    Duration {
        #[arg(value_name = "SECONDS", value_parser = parse_tenths, long_help = "Set the movement time in seconds, using a whole number or one decimal place, such as 12 or 12.3. This applies to timed Focus Preset, A-B Focus, and aperture functions. The maximum depends on the connected lens.")]
        value: u16,
    },
    /// Set the delay before a timed move starts.
    Delay {
        #[arg(value_name = "SECONDS", value_parser = parse_tenths, long_help = "Set a delay from 0.0 to 9.9 seconds before a timed focus or aperture move begins. Use a whole number or one decimal place. This setting is available only on supported lenses.")]
        value: u16,
    },
    /// Set how many time-lapse shots hold the same focus.
    SkipCount {
        #[arg(value_name = "COUNT", value_parser = clap::value_parser!(u16).range(0..=9999), long_help = "Set the number of shots taken at the same focus position before focus starts moving. This applies only when Focus Time Lapse is assigned. Accepted values are 0 through 9999.")]
        value: u16,
    },
    /// Set how many time-lapse shots are used while focus moves.
    MoveCount {
        #[arg(value_name = "COUNT", value_parser = clap::value_parser!(u16).range(0..=9999), long_help = "Set the number of shots used while focus moves to the next position. This applies only when Focus Time Lapse is assigned. Accepted values are 0 through 9999; 0 is stored as 1.")]
        value: u16,
    },
    /// Set the autofocus range used by Focus Limiter.
    AfLimit(AfLimitArgs),
}

#[derive(Debug, Args)]
struct AfLimitArgs {
    /// Numbered near-focus position.
    #[arg(
        long,
        value_name = "INDEX",
        long_help = "Choose the near end of the AF range by its number in `tlc info`. It must come before --far in that list."
    )]
    near: u8,
    /// Numbered far-focus position.
    #[arg(
        long,
        value_name = "INDEX",
        long_help = "Choose the far end of the AF range by its number in `tlc info`. It must come after --near. On lenses with a fixed infinity limit, choose the final `inf` entry."
    )]
    far: u8,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ButtonArg {
    Focus,
    #[value(name = "custom-1")]
    Custom1,
    #[value(name = "custom-2")]
    Custom2,
    #[value(name = "custom-3")]
    Custom3,
}

impl From<ButtonArg> for ButtonSlot {
    fn from(value: ButtonArg) -> Self {
        match value {
            ButtonArg::Focus => Self::Focus,
            ButtonArg::Custom1 => Self::Custom1,
            ButtonArg::Custom2 => Self::Custom2,
            ButtonArg::Custom3 => Self::Custom3,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ButtonFunctionArg {
    None,
    AfMfHold,
    AfMfPress,
    AfLimitHold,
    AfLimitPress,
    FocusHold,
    AfLimitWhilePressed,
    FullRangeWhilePressed,
    FocusPreset,
    AbFocus,
    #[value(name = "focus-hold-2")]
    FocusHold2,
    AstroFine,
    RingSwitchHold,
    RingSwitchPress,
    AstroFixedHold,
    AstroFixedPress,
    FocusStopperHold,
    FocusStopperPress,
    VcHold,
    VcPress,
    FocusTimelapseHold,
    TimedFocusPreset,
    TimedAbFocus,
    TimedIrisPreset,
    TimedAbIris,
    MfResponseHold,
    MfResponsePress,
    RingStopperHold,
    RingStopperPress,
}

impl From<ButtonFunctionArg> for ButtonFunction {
    fn from(value: ButtonFunctionArg) -> Self {
        match value {
            ButtonFunctionArg::None => Self::None,
            ButtonFunctionArg::AfMfHold => Self::AfMfHold,
            ButtonFunctionArg::AfMfPress => Self::AfMfPress,
            ButtonFunctionArg::AfLimitHold => Self::AfLimitHold,
            ButtonFunctionArg::AfLimitPress => Self::AfLimitPress,
            ButtonFunctionArg::FocusHold => Self::FocusHold,
            ButtonFunctionArg::AfLimitWhilePressed => Self::AfLimitWhilePressed,
            ButtonFunctionArg::FullRangeWhilePressed => Self::FullRangeWhilePressed,
            ButtonFunctionArg::FocusPreset => Self::FocusPreset,
            ButtonFunctionArg::AbFocus => Self::AbFocus,
            ButtonFunctionArg::FocusHold2 => Self::FocusHold2,
            ButtonFunctionArg::AstroFine => Self::AstroFine,
            ButtonFunctionArg::RingSwitchHold => Self::RingSwitchHold,
            ButtonFunctionArg::RingSwitchPress => Self::RingSwitchPress,
            ButtonFunctionArg::AstroFixedHold => Self::AstroFixedHold,
            ButtonFunctionArg::AstroFixedPress => Self::AstroFixedPress,
            ButtonFunctionArg::FocusStopperHold => Self::FocusStopperHold,
            ButtonFunctionArg::FocusStopperPress => Self::FocusStopperPress,
            ButtonFunctionArg::VcHold => Self::VcHold,
            ButtonFunctionArg::VcPress => Self::VcPress,
            ButtonFunctionArg::FocusTimelapseHold => Self::FocusTimelapseHold,
            ButtonFunctionArg::TimedFocusPreset => Self::TimedFocusPreset,
            ButtonFunctionArg::TimedAbFocus => Self::TimedAbFocus,
            ButtonFunctionArg::TimedIrisPreset => Self::TimedIrisPreset,
            ButtonFunctionArg::TimedAbIris => Self::TimedAbIris,
            ButtonFunctionArg::MfResponseHold => Self::MfResponseHold,
            ButtonFunctionArg::MfResponsePress => Self::MfResponsePress,
            ButtonFunctionArg::RingStopperHold => Self::RingStopperHold,
            ButtonFunctionArg::RingStopperPress => Self::RingStopperPress,
        }
    }
}

#[derive(Debug, Subcommand)]
enum SwitchCommand {
    /// Show the Custom Switch mode and AF ranges.
    Get,
    /// Change how the Custom Switch works.
    #[command(subcommand)]
    Set(SwitchSetCommand),
}

#[derive(Debug, Subcommand)]
enum SwitchSetCommand {
    /// Choose how the Custom Switch positions work.
    Mode {
        #[arg(
            value_enum,
            long_help = "Choose `af-limit` to give each position an AF range, `af-limit-mf` to use AF ranges on positions 1 and 2 and manual focus on position 3, or `multi-select` to assign a different shooting function to each position. Only modes supported by the connected lens can be selected."
        )]
        value: SwitchModeArg,
    },
    /// Set the AF range for one Custom Switch position.
    AfLimit {
        /// Custom Switch position number.
        #[arg(value_name = "POSITION", value_parser = clap::value_parser!(u8).range(1..=3), long_help = "Choose Custom Switch position 1, 2, or 3. The position must be present on the connected lens.")]
        position: u8,
        #[command(flatten)]
        limit: AfLimitArgs,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SwitchModeArg {
    AfLimit,
    AfLimitMf,
    MultiSelect,
}

impl From<SwitchModeArg> for SwitchMode {
    fn from(value: SwitchModeArg) -> Self {
        match value {
            SwitchModeArg::AfLimit => Self::AfLimit,
            SwitchModeArg::AfLimitMf => Self::AfLimitMf,
            SwitchModeArg::MultiSelect => Self::MultiSelect,
        }
    }
}

#[derive(Debug, Subcommand)]
enum FocusCalibrationCommand {
    /// Show the current focus correction and available range.
    Get,
    /// Fine-tune autofocus accuracy.
    Set {
        #[arg(
            value_name = "VALUE",
            allow_hyphen_values = true,
            long_help = "Choose a correction within the range shown by `tlc focus-calibration get`. Use a positive value when autofocus lands in front of the subject, or a negative value when it lands behind. Check the result with test shots at several distances."
        )]
        value: i8,
    },
}

#[derive(Debug, Subcommand)]
enum SettingsCommand {
    /// Back up all current lens settings to a file.
    Save {
        /// Backup file to create.
        #[arg(
            value_name = "FILE",
            long_help = "Save all current lens settings to a new backup file. A `.tlc` extension is recommended. tlc never replaces an existing backup; choose another file name if the path already exists."
        )]
        file: PathBuf,
    },
    /// Restore all settings from a backup file.
    Load {
        /// Backup file to restore.
        #[arg(
            value_name = "FILE",
            long_help = "Restore every lens setting from a `.tlc` backup. Before making changes, tlc checks that the file is undamaged and was created for the same lens model."
        )]
        file: PathBuf,
        /// Confirm that all current settings may be replaced.
        #[arg(
            long,
            long_help = "Restore without asking for confirmation. Use this for scripts or after reviewing the backup and current lens settings. A backup from a different firmware version of the same lens is allowed with a warning."
        )]
        yes: bool,
    },
    /// Return the lens to its factory settings.
    Reset {
        /// Confirm that all customized settings may be erased.
        #[arg(
            long,
            long_help = "Reset without asking for confirmation. Use this for scripts or when you have already decided to erase all customized lens settings."
        )]
        yes: bool,
    },
}

pub(crate) fn run() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    run_cli(cli)
}

fn run_cli(cli: Cli) -> Result<()> {
    if matches!(cli.command, Command::Devices) {
        if cli.device.is_some() {
            return Err(Error::InvalidValue(
                "--device is not valid with the devices command".into(),
            ));
        }
        return print_devices(&discover_devices()?);
    }

    let devices = discover_devices()?;
    if devices.len() > 1 && cli.device.is_none() {
        eprintln!("Compatible devices:");
        print_device_rows(&devices, true);
    }
    let device = select_device(&devices, cli.device.as_deref())?;
    log::debug!(target: "tlc", "using lens at {}", device.port_name);
    let mut lens = Lens::connect(&device)?;
    let action_result = execute(&mut lens, cli.command);
    match action_result {
        Ok(()) => lens.disconnect(),
        Err(error) => {
            let _ = lens.disconnect();
            Err(error)
        }
    }
}

fn execute(lens: &mut Lens, command: Command) -> Result<()> {
    match command {
        Command::Devices => unreachable!(),
        Command::Info => print_info(lens),
        Command::Ring(command) => execute_ring(lens, command),
        Command::Button(command) => execute_button(lens, command),
        Command::Switch(command) => execute_switch(lens, command),
        Command::FocusCalibration(command) => execute_focus_calibration(lens, command),
        Command::Settings(command) => execute_settings(lens, command),
    }
}

fn execute_ring(lens: &mut Lens, command: RingCommand) -> Result<()> {
    match command {
        RingCommand::Get { setting } => print_ring(lens, setting.map(Into::into)),
        RingCommand::Set(command) => {
            let change = match command {
                RingSetCommand::Function { value } => SettingChange::RingFunction(match value {
                    RingFunctionArg::Focus => FocusRingFunction::Focus,
                    RingFunctionArg::Aperture => FocusRingFunction::Aperture,
                }),
                RingSetCommand::Direction { value } => SettingChange::RingDirection(match value {
                    RingDirectionArg::Forward => FocusRingDirection::Forward,
                    RingDirectionArg::Reverse => FocusRingDirection::Reverse,
                    RingDirectionArg::Camera => FocusRingDirection::Camera,
                }),
                RingSetCommand::Response { value } => SettingChange::RingResponse(match value {
                    RingResponseArg::Nonlinear => FocusRingResponse::Nonlinear,
                    RingResponseArg::Linear => FocusRingResponse::Linear,
                }),
                RingSetCommand::Angle { value } => SettingChange::RingAngle(value),
                RingSetCommand::ApertureAngle { value } => SettingChange::ApertureAngle(value),
                RingSetCommand::OverrideSensitivity { value } => {
                    SettingChange::OverrideSensitivity(value)
                }
            };
            lens.apply(change)?;
            print_ring(lens, None)
        }
    }
}

fn execute_button(lens: &mut Lens, command: ButtonCommand) -> Result<()> {
    match command {
        ButtonCommand::Get { button } => print_buttons(lens, button.map(Into::into)),
        ButtonCommand::Set(args) => {
            let slot = args.button.into();
            let change = match args.setting {
                ButtonSetCommand::Function { value } => {
                    SettingChange::ButtonFunction(slot, value.into())
                }
                ButtonSetCommand::Speed { value } => SettingChange::ButtonSpeed(slot, value),
                ButtonSetCommand::Duration { value } => SettingChange::ButtonDuration(slot, value),
                ButtonSetCommand::Delay { value } => SettingChange::ButtonDelay(slot, value),
                ButtonSetCommand::SkipCount { value } => {
                    SettingChange::ButtonSkipCount(slot, value)
                }
                ButtonSetCommand::MoveCount { value } => {
                    SettingChange::ButtonMoveCount(slot, value)
                }
                ButtonSetCommand::AfLimit(limit) => {
                    SettingChange::ButtonAfLimit(slot, limit.into())
                }
            };
            lens.apply(change)?;
            print_buttons(lens, Some(slot))
        }
    }
}

fn execute_switch(lens: &mut Lens, command: SwitchCommand) -> Result<()> {
    match command {
        SwitchCommand::Get => print_switch(lens),
        SwitchCommand::Set(command) => {
            let change = match command {
                SwitchSetCommand::Mode { value } => SettingChange::SwitchMode(value.into()),
                SwitchSetCommand::AfLimit { position, limit } => {
                    SettingChange::SwitchAfLimit(position, limit.into())
                }
            };
            lens.apply(change)?;
            print_switch(lens)
        }
    }
}

fn execute_focus_calibration(lens: &mut Lens, command: FocusCalibrationCommand) -> Result<()> {
    match command {
        FocusCalibrationCommand::Get => print_focus_calibration(lens),
        FocusCalibrationCommand::Set { value } => {
            lens.apply(SettingChange::FocusAdjustment(value))?;
            print_focus_calibration(lens)
        }
    }
}

fn execute_settings(lens: &mut Lens, command: SettingsCommand) -> Result<()> {
    match command {
        SettingsCommand::Save { file } => {
            lens.snapshot().write_to(&file)?;
            println!("saved settings: {}", file.display());
        }
        SettingsCommand::Load { file, yes } => {
            let snapshot = SettingsSnapshot::read_from(&file)?;
            if snapshot.model_id() != &lens.info().model_id {
                return Err(Error::SnapshotModelMismatch);
            }
            if snapshot.firmware_version()
                != (lens.info().firmware_major, lens.info().firmware_minor)
            {
                let (major, minor) = snapshot.firmware_version();
                eprintln!(
                    "tlc: warning: this backup was made with firmware {major:02X}.{minor:02X}; the lens is running {}",
                    lens.info().firmware_version()
                );
            }
            if !yes
                && !prompt_for_confirmation(
                    "Restore this backup and replace every current lens setting?",
                )?
            {
                eprintln!("tlc: cancelled; lens settings were not changed");
                return Ok(());
            }
            lens.restore_snapshot(&snapshot)?;
            println!("loaded settings: {}", file.display());
        }
        SettingsCommand::Reset { yes } => {
            if !yes
                && !prompt_for_confirmation("Reset the lens and erase every customized setting?")?
            {
                eprintln!("tlc: cancelled; lens settings were not changed");
                return Ok(());
            }
            lens.factory_reset()?;
            println!("factory settings restored");
        }
    }
    Ok(())
}

fn prompt_for_confirmation(question: &str) -> Result<bool> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stderr = io::stderr();
    let mut output = stderr.lock();
    read_confirmation(&mut input, &mut output, question).map_err(Error::Io)
}

fn read_confirmation(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &str,
) -> io::Result<bool> {
    write!(output, "{question} Type \"yes\" to continue: ")?;
    output.flush()?;

    let mut answer = String::new();
    if input.read_line(&mut answer)? == 0 {
        writeln!(output)?;
        return Ok(false);
    }
    Ok(answer.trim().eq_ignore_ascii_case("yes"))
}

impl From<AfLimitArgs> for AfLimit {
    fn from(value: AfLimitArgs) -> Self {
        Self {
            near_index: value.near,
            far_index: value.far,
        }
    }
}

fn print_devices(devices: &[tamron_lens_control::DeviceInfo]) -> Result<()> {
    if devices.is_empty() {
        println!("No compatible Tamron lenses found.");
        print_driver_guidance();
    } else {
        println!("SERIAL\tPORT\tUSB");
        print_device_rows(devices, false);
    }
    Ok(())
}

fn print_device_rows(devices: &[tamron_lens_control::DeviceInfo], stderr: bool) {
    for device in devices {
        let serial = device.serial_number.as_deref().unwrap_or("-");
        let line = format!(
            "{serial}\t{}\t{:04x}:{:04x}",
            device.port_name, device.vendor_id, device.product_id
        );
        if stderr {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }
}

pub(crate) fn print_driver_guidance() {
    eprintln!("The cp210x driver may need these runtime registrations:");
    eprintln!("  sudo modprobe cp210x");
    eprintln!("  echo 2cd1 0002 | sudo tee /sys/bus/usb-serial/drivers/cp210x/new_id");
    eprintln!("  echo 2cd1 0005 | sudo tee /sys/bus/usb-serial/drivers/cp210x/new_id");
    eprintln!("Reconnect the lens, then run `tlc devices` again.");
}

fn print_info(lens: &Lens) -> Result<()> {
    let info = lens.info();
    println!("Product: {}", info.product_name);
    println!("Model: {}", info.model_name);
    println!("Mount: {}", info.mount);
    println!("Class: {}", info.lens_class);
    println!("Firmware: {}", info.firmware_version());
    println!("Connection: {}", info.connection_state);
    println!("Focus-set buttons: {}", info.focus_button_count);
    println!("Custom-switch positions: {}", info.switch_position_count);
    println!("Supported button functions:");
    for value in ButtonFunctionArg::value_variants() {
        let function: ButtonFunction = (*value).into();
        if info.capabilities.supports_button_function(function) {
            println!("  {}", value.to_possible_value().unwrap().get_name());
        }
    }
    println!("AF-limit positions:");
    for position in &info.limit_positions {
        println!("  {}\t{}", position.index, position.label);
    }
    println!("Current settings:");
    print_ring(lens, None)?;
    if info.capabilities.supports_feature(1) {
        print_switch(lens)?;
    }
    print_buttons(lens, None)?;
    if info.capabilities.supports_feature(7) {
        print_focus_calibration(lens)?;
    }
    Ok(())
}

fn print_ring(lens: &Lens, selected: Option<RingSetting>) -> Result<()> {
    let info = lens.info();
    let settings = lens.settings();
    let rows = [
        (
            RingSetting::Function,
            2,
            "ring.function",
            settings.ring_function.to_string(),
        ),
        (
            RingSetting::Direction,
            3,
            "ring.direction",
            settings.ring_direction.to_string(),
        ),
        (
            RingSetting::Response,
            4,
            "ring.response",
            settings.ring_response.to_string(),
        ),
        (
            RingSetting::Angle,
            5,
            "ring.angle",
            format!("{} degrees", settings.ring_angle),
        ),
        (
            RingSetting::ApertureAngle,
            9,
            "ring.aperture-angle",
            format!("{} degrees", settings.aperture_angle),
        ),
        (
            RingSetting::OverrideSensitivity,
            12,
            "ring.override-sensitivity",
            settings.override_sensitivity.to_string(),
        ),
    ];
    let mut printed = false;
    for (setting, bit, label, value) in rows {
        if selected.is_none_or(|selected| selected == setting)
            && info.capabilities.supports_feature(bit)
        {
            println!("{label}: {value}");
            printed = true;
        }
    }
    if selected.is_some() && !printed {
        return Err(Error::UnsupportedSetting("requested ring setting".into()));
    }
    Ok(())
}

fn print_buttons(lens: &Lens, selected: Option<ButtonSlot>) -> Result<()> {
    let mut printed = false;
    for slot in [
        ButtonSlot::Focus,
        ButtonSlot::Custom1,
        ButtonSlot::Custom2,
        ButtonSlot::Custom3,
    ] {
        if selected.is_none_or(|selected| selected == slot) && lens.supports_slot(slot) {
            print_button(lens, slot);
            printed = true;
        }
    }
    if selected.is_some() && !printed {
        return Err(Error::UnsupportedSetting("requested button slot".into()));
    }
    Ok(())
}

fn print_button(lens: &Lens, slot: ButtonSlot) {
    let button = &lens.settings().buttons[slot.index()];
    println!("button.{slot}.function: {}", button.function);
    match button.function {
        ButtonFunction::FocusPreset | ButtonFunction::AbFocus => {
            println!("button.{slot}.speed: {}", button.speed);
        }
        ButtonFunction::TimedFocusPreset
        | ButtonFunction::TimedAbFocus
        | ButtonFunction::TimedIrisPreset
        | ButtonFunction::TimedAbIris => {
            println!(
                "button.{slot}.duration: {}.{} seconds",
                button.duration_tenths / 10,
                button.duration_tenths % 10
            );
            if lens.info().capabilities.supports_feature(10) {
                println!(
                    "button.{slot}.delay: {}.{} seconds",
                    button.delay_tenths / 10,
                    button.delay_tenths % 10
                );
            }
        }
        ButtonFunction::FocusTimelapseHold => {
            println!("button.{slot}.skip-count: {}", button.skip_count);
            println!("button.{slot}.move-count: {}", button.move_count);
        }
        ButtonFunction::AfLimitHold
        | ButtonFunction::AfLimitPress
        | ButtonFunction::AfLimitWhilePressed
        | ButtonFunction::FullRangeWhilePressed => {
            print_limit(&format!("button.{slot}.af-limit"), button.af_limit, lens);
        }
        _ => {}
    }
}

fn print_switch(lens: &Lens) -> Result<()> {
    if !lens.info().capabilities.supports_feature(1) {
        return Err(Error::UnsupportedSetting("custom switch".into()));
    }
    println!("switch.mode: {}", lens.settings().switch_mode);
    print!("switch.supported-modes:");
    for mode in [
        SwitchMode::AfLimit,
        SwitchMode::AfLimitMf,
        SwitchMode::MultiSelect,
    ] {
        if lens.info().capabilities.supports_switch_mode(mode) {
            print!(" {mode}");
        }
    }
    println!();
    for position in 0..usize::from(lens.info().switch_position_count) {
        print_limit(
            &format!("switch.position-{}.af-limit", position + 1),
            lens.settings().switch_limits[position],
            lens,
        );
    }
    Ok(())
}

fn print_focus_calibration(lens: &Lens) -> Result<()> {
    if !lens.info().capabilities.supports_feature(7) {
        return Err(Error::UnsupportedSetting("focus-point adjustment".into()));
    }
    println!(
        "focus-calibration: {} (range {}..{})",
        lens.settings().focus_adjustment,
        -lens.info().calibration_half_range,
        lens.info().calibration_half_range
    );
    Ok(())
}

fn print_limit(label: &str, limit: AfLimit, lens: &Lens) {
    let positions = &lens.info().limit_positions;
    println!(
        "{label}: near {} ({}) far {} ({})",
        limit.near_index,
        positions[usize::from(limit.near_index)].label,
        limit.far_index,
        positions[usize::from(limit.far_index)].label
    );
}

fn parse_tenths(value: &str) -> std::result::Result<u16, String> {
    if value.starts_with('-') || value.is_empty() {
        return Err("expected a non-negative number of seconds".into());
    }
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) if fraction.len() == 1 => (whole, fraction),
        Some(_) => {
            return Err("seconds must have exactly one digit after the decimal point".into());
        }
        None => (value, "0"),
    };
    if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid seconds value".into());
    }
    let whole = whole
        .parse::<u16>()
        .map_err(|_| "seconds value is too large".to_owned())?;
    let fraction = fraction.as_bytes()[0] - b'0';
    whole
        .checked_mul(10)
        .and_then(|value| value.checked_add(u16::from(fraction)))
        .ok_or_else(|| "seconds value is too large".into())
}

struct CliLogger;

impl Log for CliLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target() == "tlc" && metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("tlc: {}", record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: CliLogger = CliLogger;
static LOGGER_INIT: Once = Once::new();

fn init_logging(verbosity: u8) {
    let level = match verbosity {
        0 => LevelFilter::Off,
        1 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };
    LOGGER_INIT.call_once(|| {
        let _ = log::set_logger(&LOGGER);
    });
    log::set_max_level(level);
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn parses_locked_command_shapes() {
        Cli::try_parse_from([
            "tlc",
            "-d",
            "/dev/ttyUSB0",
            "ring",
            "set",
            "direction",
            "reverse",
        ])
        .unwrap();
        Cli::try_parse_from([
            "tlc", "button", "set", "custom-2", "af-limit", "--near", "1", "--far", "3",
        ])
        .unwrap();
        Cli::try_parse_from(["tlc", "settings", "load", "backup.tlc", "--yes"]).unwrap();
        Cli::try_parse_from(["tlc", "focus-calibration", "set", "-2"]).unwrap();
        assert!(Cli::try_parse_from(["tlc", "adjustment", "get"]).is_err());
    }

    #[test]
    fn verbosity_counts_short_and_long_flags() {
        assert_eq!(
            Cli::try_parse_from(["tlc", "-v", "info"]).unwrap().verbose,
            1
        );
        assert_eq!(
            Cli::try_parse_from(["tlc", "--verbose", "--verbose", "info"])
                .unwrap()
                .verbose,
            2
        );
        assert!(Cli::try_parse_from(["tlc", "info", "-vv"]).is_err());
    }

    #[test]
    fn destructive_commands_allow_prompt_or_yes() {
        Cli::try_parse_from(["tlc", "settings", "reset"]).unwrap();
        Cli::try_parse_from(["tlc", "settings", "reset", "--yes"]).unwrap();
        Cli::try_parse_from(["tlc", "settings", "load", "backup.tlc"]).unwrap();
        assert!(Cli::try_parse_from(["tlc", "settings", "save", "backup.tlc", "--force"]).is_err());
    }

    #[test]
    fn confirmation_requires_the_full_word_yes() {
        for answer in ["yes\n", "YES\n", " yes \n"] {
            let mut input = io::Cursor::new(answer);
            let mut output = Vec::new();
            assert!(read_confirmation(&mut input, &mut output, "Proceed?").unwrap());
            assert_eq!(
                String::from_utf8(output).unwrap(),
                "Proceed? Type \"yes\" to continue: "
            );
        }

        for answer in ["y\n", "no\n", "\n", ""] {
            let mut input = io::Cursor::new(answer);
            let mut output = Vec::new();
            assert!(!read_confirmation(&mut input, &mut output, "Proceed?").unwrap());
        }
    }

    #[test]
    fn decimal_seconds_are_exact() {
        assert_eq!(parse_tenths("12").unwrap(), 120);
        assert_eq!(parse_tenths("12.3").unwrap(), 123);
        assert!(parse_tenths("12.34").is_err());
    }

    #[test]
    fn clap_tree_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn button_function_help_uses_user_facing_language() {
        let mut command = Cli::command();
        let function = command
            .find_subcommand_mut("button")
            .unwrap()
            .find_subcommand_mut("set")
            .unwrap()
            .find_subcommand_mut("function")
            .unwrap();
        let help = function.render_long_help().to_string();
        assert!(help.contains("Choose the shooting function"));
        assert!(!help.contains("semantic function"));
        assert!(!help.contains("capability bits"));
        assert!(!help.contains("logical slot"));
    }
}
