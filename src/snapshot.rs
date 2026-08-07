use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use crc::{CRC_32_ISO_HDLC, Crc};

use crate::{Error, Result};

const MAGIC: [u8; 8] = *b"TLCSET\0\0";
const VERSION: u16 = 1;
const HEADER_LENGTH: u16 = 32;
const SETTINGS_LENGTH: u32 = 512;
const FILE_LENGTH: usize = 548;
const SNAPSHOT_CRC: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

/// A versioned, integrity-checked, lossless lens settings image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsSnapshot {
    model_id: [u8; 8],
    firmware_major: u8,
    firmware_minor: u8,
    settings: [u8; 512],
}

impl SettingsSnapshot {
    pub(crate) fn new(
        model_id: [u8; 8],
        firmware_major: u8,
        firmware_minor: u8,
        settings: [u8; 512],
    ) -> Self {
        Self {
            model_id,
            firmware_major,
            firmware_minor,
            settings,
        }
    }

    /// Read and validate a snapshot from disk.
    pub fn read_from(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|source| Error::SnapshotIo {
            path: path.to_path_buf(),
            source,
        })?;
        Self::decode(&bytes)
    }

    /// Write the snapshot to a new file without replacing existing data.
    pub fn write_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let bytes = self.encode();
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(path).map_err(|source| Error::SnapshotIo {
            path: path.to_path_buf(),
            source,
        })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| Error::SnapshotIo {
                path: path.to_path_buf(),
                source,
            })
    }

    /// Raw eight-byte model identifier stored in the snapshot.
    pub fn model_id(&self) -> &[u8; 8] {
        &self.model_id
    }

    /// Firmware bytes present when the snapshot was created.
    pub fn firmware_version(&self) -> (u8, u8) {
        (self.firmware_major, self.firmware_minor)
    }

    pub(crate) fn settings(&self) -> &[u8; 512] {
        &self.settings
    }

    fn encode(&self) -> [u8; FILE_LENGTH] {
        let mut bytes = [0_u8; FILE_LENGTH];
        bytes[0..8].copy_from_slice(&MAGIC);
        bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
        bytes[10..12].copy_from_slice(&HEADER_LENGTH.to_le_bytes());
        bytes[12..16].copy_from_slice(&SETTINGS_LENGTH.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.model_id);
        bytes[24] = self.firmware_major;
        bytes[25] = self.firmware_minor;
        bytes[32..544].copy_from_slice(&self.settings);
        let checksum = SNAPSHOT_CRC.checksum(&bytes[..544]);
        bytes[544..548].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != FILE_LENGTH {
            return Err(Error::InvalidSnapshot(format!(
                "expected {FILE_LENGTH} bytes, got {}",
                bytes.len()
            )));
        }
        if bytes[..8] != MAGIC {
            return Err(Error::InvalidSnapshot("unrecognized file magic".into()));
        }
        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version != VERSION {
            return Err(Error::InvalidSnapshot(format!(
                "unsupported format version {version}"
            )));
        }
        if u16::from_le_bytes([bytes[10], bytes[11]]) != HEADER_LENGTH
            || u32::from_le_bytes(bytes[12..16].try_into().unwrap()) != SETTINGS_LENGTH
        {
            return Err(Error::InvalidSnapshot(
                "invalid header or payload length".into(),
            ));
        }
        if bytes[26..32].iter().any(|byte| *byte != 0) {
            return Err(Error::InvalidSnapshot(
                "reserved header bytes are nonzero".into(),
            ));
        }
        let expected = SNAPSHOT_CRC.checksum(&bytes[..544]);
        let received = u32::from_le_bytes(bytes[544..548].try_into().unwrap());
        if expected != received {
            return Err(Error::InvalidSnapshot(format!(
                "CRC mismatch: expected 0x{expected:08X}, got 0x{received:08X}"
            )));
        }
        Ok(Self {
            model_id: bytes[16..24].try_into().unwrap(),
            firmware_major: bytes[24],
            firmware_minor: bytes[25],
            settings: bytes[32..544].try_into().unwrap(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, process};

    fn snapshot() -> SettingsSnapshot {
        SettingsSnapshot::new(*b"A067\0\0\0\0", 1, 2, [0x5a; 512])
    }

    #[test]
    fn round_trips_exact_container() {
        let original = snapshot();
        let bytes = original.encode();
        assert_eq!(bytes.len(), FILE_LENGTH);
        assert_eq!(SettingsSnapshot::decode(&bytes).unwrap(), original);
    }

    #[test]
    fn rejects_corruption() {
        let mut bytes = snapshot().encode();
        bytes[100] ^= 1;
        assert!(matches!(
            SettingsSnapshot::decode(&bytes),
            Err(Error::InvalidSnapshot(message)) if message.contains("CRC")
        ));
    }

    #[test]
    fn refuses_to_replace_an_existing_snapshot() {
        let path = env::temp_dir().join(format!("tlc-snapshot-test-{}.tlc", process::id()));
        let original = snapshot();
        original.write_to(&path).unwrap();
        let error = original.write_to(&path).unwrap_err();
        assert!(matches!(
            error,
            Error::SnapshotIo { source, .. }
                if source.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(SettingsSnapshot::read_from(&path).unwrap(), original);
        fs::remove_file(path).unwrap();
    }
}
