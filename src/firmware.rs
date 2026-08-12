use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use crc::{CRC_16_XMODEM, Crc};
use quick_xml::{Reader, events::Event, name::QName};
use reqwest::{StatusCode, blocking::Client, redirect};

use crate::{Error, LensInfo, Mount, Result};

const BASE_URL: &str = "https://tamron.cdngc.net/lensutility/lens/";
const CONTAINER_CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_XMODEM);
const BASE_KEY: &[u8; 32] = b"EncryptionToolBaseKeyEncryptionT";
const HEADER_SIZE: usize = 1024;
const MAX_PAYLOAD_SIZE: usize = 1024 * 1024;

/// Metadata advertised by Tamron's firmware service for a connected lens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirmwareMetadata {
    /// Connected lens model identifier.
    pub model: String,
    /// Connected lens mount.
    pub mount: Mount,
    /// Protocol-defined display form of the installed version.
    pub installed_version: String,
    /// Protocol-defined display form of the advertised version.
    pub available_version: String,
    pub(crate) firmware_file_name: String,
    pub(crate) description_key: String,
}

impl FirmwareMetadata {
    /// Whether the service advertises usable metadata for this model and mount.
    pub fn is_available(&self) -> bool {
        self.available_version != "--"
    }

    /// Whether the advertised and installed display versions differ.
    pub fn update_indicated(&self) -> bool {
        self.is_available() && self.available_version != self.installed_version
    }
}

/// Progress emitted by a firmware update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FirmwareProgress {
    /// The encrypted firmware payload is being downloaded.
    Downloading,
    /// The complete payload is being decoded and CRC-checked.
    Validating,
    /// One selected image is about to enter firmware-transfer mode.
    ImageStarted {
        /// One-based image index.
        image: usize,
        /// Total selected image count.
        image_count: usize,
        /// Image device selector.
        device: u8,
        /// Effective image area selector.
        area: u8,
    },
    /// A 1024-byte transfer block was acknowledged by the lens.
    BlockAcknowledged {
        /// Acknowledged bytes including initial and padded blocks.
        acknowledged_bytes: u64,
        /// Total padded bytes that will be transmitted.
        total_bytes: u64,
        /// Integer percentage from zero through 100.
        percent: u8,
    },
    /// One selected image completed its EOT handshake.
    ImageCompleted {
        /// One-based image index.
        image: usize,
        /// Total selected image count.
        image_count: usize,
    },
}

/// Result of a completed high-level firmware-update request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirmwareUpdateOutcome {
    /// Every selected image completed its lens handshake.
    Installed,
    /// The installed and advertised display versions matched.
    UpToDate,
    /// The caller declined the update after inspecting its metadata.
    Declined,
}

/// Shared cancellation and protected-transfer state for a firmware update.
#[derive(Clone, Debug, Default)]
pub struct FirmwareUpdateControl {
    state: Arc<AtomicU8>,
}

impl FirmwareUpdateControl {
    /// Create a new independent update-control token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. The updater honors this only before transfer starts.
    pub fn cancel(&self) {
        let _ = self
            .state
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Whether the updater is in the protected lens-transfer phase.
    pub fn transfer_active(&self) -> bool {
        self.state.load(Ordering::SeqCst) == 2
    }

    pub(crate) fn check_cancelled(&self) -> Result<()> {
        if self.state.load(Ordering::SeqCst) == 1 {
            Err(Error::FirmwareCancelled)
        } else {
            Ok(())
        }
    }

    pub(crate) fn protect_transfer(&self) -> Result<()> {
        self.state
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| Error::FirmwareCancelled)
    }

    pub(crate) fn finish_transfer(&self) {
        self.state.store(0, Ordering::SeqCst);
    }
}

#[derive(Debug)]
pub(crate) struct FirmwareImage {
    pub(crate) device: u8,
    pub(crate) area: u8,
    pub(crate) payload: Vec<u8>,
}

pub(crate) fn fetch_metadata(info: &LensInfo) -> Result<FirmwareMetadata> {
    let model = info.model_name.clone();
    let suffix = match info.mount {
        Mount::SonyE => "SE",
        Mount::CanonRf => "RF",
        Mount::NikonZ => "Z",
    };
    let url = format!("{BASE_URL}{model}{suffix}.xml");
    log::debug!(target: "tlc", "fetching firmware metadata from {url}");
    let response = client(Duration::from_secs(60))?
        .get(url)
        .send()
        .map_err(|error| Error::FirmwareMetadata(error.to_string()))?;

    let (version, firmware_file_name, description_key) =
        if response.status() == StatusCode::NOT_FOUND {
            ("--".into(), String::new(), String::new())
        } else {
            let response = response
                .error_for_status()
                .map_err(|error| Error::FirmwareMetadata(error.to_string()))?;
            let bytes = response
                .bytes()
                .map_err(|error| Error::FirmwareMetadata(error.to_string()))?;
            log::trace!(
                target: "tlc",
                "firmware metadata XML:\n{}",
                String::from_utf8_lossy(&bytes)
            );
            parse_manifest(&bytes)?
        };

    Ok(FirmwareMetadata {
        model,
        mount: info.mount,
        installed_version: installed_display(info.firmware_major, info.firmware_minor),
        available_version: advertised_display(&version),
        firmware_file_name,
        description_key,
    })
}

pub(crate) fn download_and_decode(
    metadata: &FirmwareMetadata,
    control: &FirmwareUpdateControl,
    progress: &mut impl FnMut(FirmwareProgress),
) -> Result<Vec<FirmwareImage>> {
    validate_metadata(metadata)?;
    control.check_cancelled()?;
    progress(FirmwareProgress::Downloading);
    let url = format!("{BASE_URL}{}", metadata.firmware_file_name);
    log::debug!(target: "tlc", "downloading firmware payload from {url}");
    let mut response = client(Duration::from_secs(180))?
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| Error::FirmwareDownload(error.to_string()))?;
    let mut encoded = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        control.check_cancelled()?;
        let count = response
            .read(&mut buffer)
            .map_err(|error| Error::FirmwareDownload(error.to_string()))?;
        if count == 0 {
            break;
        }
        encoded.extend_from_slice(&buffer[..count]);
    }
    control.check_cancelled()?;
    progress(FirmwareProgress::Validating);
    decode_container(&encoded, &metadata.description_key)
}

pub(crate) fn order_images(mut images: Vec<FirmwareImage>) -> Vec<FirmwareImage> {
    if images.len() > 1
        && let Some(index) = images
            .iter()
            .position(|image| image.device == 0 && image.area == 0)
    {
        let primary = images.remove(index);
        images.push(primary);
    }
    images
}

pub(crate) fn padded_transfer_size(images: &[FirmwareImage]) -> u64 {
    images
        .iter()
        .map(|image| {
            let payload_blocks = image.payload.len().div_ceil(HEADER_SIZE);
            ((payload_blocks + 1) * HEADER_SIZE) as u64
        })
        .sum()
}

fn client(timeout: Duration) -> Result<Client> {
    let redirect = redirect::Policy::custom(|attempt| {
        if attempt.url().scheme() != "https" {
            attempt.error("firmware server redirect attempted to leave HTTPS")
        } else if attempt.previous().len() >= 10 {
            attempt.error("too many firmware server redirects")
        } else {
            attempt.follow()
        }
    });
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .redirect(redirect)
        .build()
        .map_err(|error| Error::FirmwareMetadata(error.to_string()))
}

fn parse_manifest(bytes: &[u8]) -> Result<(String, String, String)> {
    let mut reader = Reader::from_reader(bytes);
    let mut version = "--".to_owned();
    let mut file_name = String::new();
    let mut key = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => match start.name().as_ref() {
                b"Version" => version = read_text(&mut reader, QName(b"Version"))?,
                b"FirmwareFileName" => {
                    file_name = read_text(&mut reader, QName(b"FirmwareFileName"))?
                }
                b"DescriptionKey" => key = read_text(&mut reader, QName(b"DescriptionKey"))?,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(Error::FirmwareMetadata(error.to_string())),
        }
    }
    Ok((version, file_name, key))
}

fn read_text(reader: &mut Reader<&[u8]>, end: QName<'_>) -> Result<String> {
    let text = reader
        .read_text(end)
        .map_err(|error| Error::FirmwareMetadata(error.to_string()))?;
    text.decode()
        .map(|text| text.into_owned())
        .map_err(|error| Error::FirmwareMetadata(error.to_string()))
}

fn installed_display(major: u8, minor: u8) -> String {
    if major >= 1 {
        format!("{major:02X}")
    } else {
        format!("{major:02X}.{minor:02X}")
    }
}

fn advertised_display(version: &str) -> String {
    match version.split_once('.') {
        Some((first, _)) if first != "0" && first != "00" => first.to_owned(),
        _ => version.to_owned(),
    }
}

fn validate_metadata(metadata: &FirmwareMetadata) -> Result<()> {
    if metadata.firmware_file_name.is_empty() {
        return Err(Error::FirmwareData(
            "manifest has no firmware filename".into(),
        ));
    }
    derive_key(&metadata.description_key).map(|_| ())
}

fn derive_key(description_key: &str) -> Result<[u8; 32]> {
    if description_key.len() != 64
        || !description_key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
    {
        return Err(Error::FirmwareData(
            "description key must be 64 uppercase hexadecimal characters".into(),
        ));
    }
    let mut key = [0_u8; 32];
    for (index, output) in key.iter_mut().enumerate() {
        let encoded = u8::from_str_radix(&description_key[index * 2..index * 2 + 2], 16)
            .map_err(|error| Error::FirmwareData(error.to_string()))?;
        *output = encoded ^ BASE_KEY[index];
    }
    Ok(key)
}

fn decode_container(encoded: &[u8], description_key: &str) -> Result<Vec<FirmwareImage>> {
    let key = derive_key(description_key)?;
    let mut key_index = 0_usize;
    let mut cursor = 0_usize;
    let mut images = Vec::new();
    while cursor < encoded.len() {
        if encoded.len() - cursor < HEADER_SIZE {
            return Err(Error::FirmwareData(
                "container ends in a partial header".into(),
            ));
        }
        let mut header = encoded[cursor..cursor + HEADER_SIZE].to_vec();
        xor(&mut header, &key, &mut key_index);
        cursor += HEADER_SIZE;
        let size = u32::from_le_bytes(header[64..68].try_into().unwrap()) as usize;
        if size > MAX_PAYLOAD_SIZE {
            return Err(Error::FirmwareData(format!(
                "record payload {size} exceeds the 1 MiB limit"
            )));
        }
        if encoded.len() - cursor < size {
            return Err(Error::FirmwareData(
                "container ends in a partial payload".into(),
            ));
        }
        let mut payload = encoded[cursor..cursor + size].to_vec();
        xor(&mut payload, &key, &mut key_index);
        cursor += size;
        let expected = u16::from_le_bytes(header[70..72].try_into().unwrap());
        let actual = CONTAINER_CRC.checksum(&payload);
        if actual != expected {
            return Err(Error::FirmwareData(format!(
                "payload CRC mismatch: expected 0x{expected:04X}, got 0x{actual:04X}"
            )));
        }
        images.push(FirmwareImage {
            device: header[72],
            area: header[73],
            payload,
        });
    }
    if images.is_empty() {
        return Err(Error::FirmwareData("container contains no images".into()));
    }
    Ok(images)
}

fn xor(bytes: &mut [u8], key: &[u8; 32], key_index: &mut usize) {
    for byte in bytes {
        *byte ^= key[*key_index];
        *key_index = (*key_index + 1) % key.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "045E554A2A3502595C5E642E5F5A7A3236137B5649752F53444123311F5F5D64";

    fn encode(images: &[(u8, u8, &[u8])]) -> Vec<u8> {
        let key = derive_key(KEY).unwrap();
        let mut key_index = 0;
        let mut encoded = Vec::new();
        for (device, area, payload) in images {
            let mut header = [0_u8; HEADER_SIZE];
            header[64..68].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            header[70..72].copy_from_slice(&CONTAINER_CRC.checksum(payload).to_le_bytes());
            header[72] = *device;
            header[73] = *area;
            xor(&mut header, &key, &mut key_index);
            let mut payload = payload.to_vec();
            xor(&mut payload, &key, &mut key_index);
            encoded.extend_from_slice(&header);
            encoded.extend_from_slice(&payload);
        }
        encoded
    }

    #[test]
    fn parses_manifest_and_display_versions() {
        let xml = b"<Root><Ignored>x</Ignored><Version>03.00</Version><FirmwareFileName>a.tfwf</FirmwareFileName><DescriptionKey>ABC</DescriptionKey></Root>";
        assert_eq!(
            parse_manifest(xml).unwrap(),
            ("03.00".into(), "a.tfwf".into(), "ABC".into())
        );
        assert_eq!(advertised_display("03.00"), "03");
        assert_eq!(advertised_display("00.12"), "00.12");
        assert_eq!(installed_display(3, 0), "03");
        assert_eq!(installed_display(0, 2), "00.02");
    }

    #[test]
    fn decodes_multiple_records_with_continuous_key() {
        let encoded = encode(&[(1, 0, b"odd"), (0, 0, b"primary")]);
        let images = decode_container(&encoded, KEY).unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].payload, b"odd");
        assert_eq!(images[1].payload, b"primary");
        let ordered = order_images(images);
        assert_eq!((ordered[1].device, ordered[1].area), (0, 0));
    }

    #[test]
    fn rejects_bad_keys_partial_data_and_crc() {
        assert!(derive_key(&KEY.to_ascii_lowercase()).is_err());
        assert!(decode_container(&[0; 12], KEY).is_err());
        let mut encoded = encode(&[(0, 0, b"payload")]);
        *encoded.last_mut().unwrap() ^= 1;
        assert!(decode_container(&encoded, KEY).is_err());
    }

    #[test]
    fn padded_progress_size_finishes_on_block_boundary() {
        let images = vec![FirmwareImage {
            device: 0,
            area: 0,
            payload: vec![0; 1025],
        }];
        assert_eq!(padded_transfer_size(&images), 3072);
    }
}
