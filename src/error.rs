use std::{io, path::PathBuf};

/// Result type used by the library.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced while discovering, communicating with, or configuring a lens.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Serial device enumeration failed.
    #[error("failed to enumerate serial devices: {0}")]
    DeviceEnumeration(#[source] serialport::Error),
    /// No compatible device matched the request.
    #[error("no compatible Tamron lens device found")]
    NoDevice,
    /// More than one compatible device requires explicit selection.
    #[error("multiple compatible Tamron lens devices found; select one with --device")]
    AmbiguousDevice,
    /// A supplied selector did not match a compatible port.
    #[error("device selector {0:?} did not match a compatible Tamron lens")]
    SelectorNotFound(String),
    /// A supplied selector matched more than one port.
    #[error("device selector {0:?} matched more than one compatible Tamron lens")]
    AmbiguousSelector(String),
    /// Opening or configuring the serial port failed.
    #[error("failed to open serial port {port:?}: {source}")]
    OpenPort {
        /// Port that could not be opened.
        port: String,
        /// Serial-port error.
        #[source]
        source: serialport::Error,
    },
    /// An operating-system I/O operation failed.
    #[error("serial I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The lens did not respond before the protocol deadline.
    #[error("communication with the lens timed out")]
    Timeout,
    /// A response frame did not satisfy the protocol.
    #[error("invalid response from lens: {0}")]
    InvalidResponse(String),
    /// The lens emitted the communication-error response opcode.
    #[error("lens returned a communication-error response")]
    CommunicationError,
    /// A memory operation was rejected by the lens.
    #[error("lens rejected the operation with result 0x{code:02X}: {message}")]
    OperationRejected {
        /// Raw result code.
        code: u8,
        /// Contextual explanation.
        message: &'static str,
    },
    /// The connect result was not supported.
    #[error("lens returned unsupported connection state 0x{0:02X}")]
    UnsupportedConnectionState(u8),
    /// The lens is waiting for firmware recovery, which v1 does not implement.
    #[error("lens is in firmware recovery mode; firmware recovery is not supported")]
    RecoveryMode,
    /// Descriptor or settings data was too short or internally inconsistent.
    #[error("invalid lens data: {0}")]
    InvalidLensData(String),
    /// The connected lens does not advertise a requested capability.
    #[error("setting is unsupported by this lens: {0}")]
    UnsupportedSetting(String),
    /// A semantic value was outside the connected lens's valid range.
    #[error("invalid setting value: {0}")]
    InvalidValue(String),
    /// A setting is not meaningful for the slot's current function.
    #[error("setting does not apply: {0}")]
    InapplicableSetting(String),
    /// Two active semantic settings share the same storage bytes.
    #[error("overlapping settings conflict: {0}")]
    OverlappingSettings(String),
    /// Reading or writing a settings snapshot failed.
    #[error("snapshot file {path:?}: {source}")]
    SnapshotIo {
        /// Snapshot path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// Snapshot bytes were malformed or failed integrity checks.
    #[error("invalid settings snapshot: {0}")]
    InvalidSnapshot(String),
    /// Snapshot belongs to a different lens model.
    #[error("snapshot model does not match the connected lens")]
    SnapshotModelMismatch,
}
