use crate::{Error, Result};

const TAMRON_VENDOR_ID: u16 = 0x2cd1;
const TAMRON_PRODUCT_IDS: [u16; 2] = [0x0002, 0x0005];

/// A compatible Linux serial device discovered by VID/PID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    /// Linux serial device path, such as `/dev/ttyUSB0`.
    pub port_name: String,
    /// USB serial string when the device exposes one.
    pub serial_number: Option<String>,
    /// USB vendor identifier.
    pub vendor_id: u16,
    /// USB product identifier.
    pub product_id: u16,
}

/// Enumerate serial ports matching the two protocol-defined Tamron USB IDs.
pub fn discover_devices() -> Result<Vec<DeviceInfo>> {
    log::debug!(target: "tlc", "searching for compatible Tamron lenses");
    let mut devices = serialport::available_ports()
        .map_err(Error::DeviceEnumeration)?
        .into_iter()
        .filter_map(|port| match port.port_type {
            serialport::SerialPortType::UsbPort(usb)
                if usb.vid == TAMRON_VENDOR_ID && TAMRON_PRODUCT_IDS.contains(&usb.pid) =>
            {
                Some(DeviceInfo {
                    port_name: port.port_name,
                    serial_number: usb.serial_number.filter(|value| !value.is_empty()),
                    vendor_id: usb.vid,
                    product_id: usb.pid,
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.port_name.cmp(&right.port_name));
    Ok(devices)
}

/// Select one device by USB serial or exact port path, or auto-select a sole device.
pub fn select_device(devices: &[DeviceInfo], selector: Option<&str>) -> Result<DeviceInfo> {
    match selector {
        None => match devices {
            [] => Err(Error::NoDevice),
            [device] => Ok(device.clone()),
            _ => Err(Error::AmbiguousDevice),
        },
        Some(selector) => {
            let matches = devices
                .iter()
                .filter(|device| {
                    device.port_name == selector
                        || device.serial_number.as_deref() == Some(selector)
                })
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [] => Err(Error::SelectorNotFound(selector.to_owned())),
                [device] => Ok((*device).clone()),
                _ => Err(Error::AmbiguousSelector(selector.to_owned())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(path: &str, serial: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            port_name: path.into(),
            serial_number: serial.map(str::to_owned),
            vendor_id: TAMRON_VENDOR_ID,
            product_id: TAMRON_PRODUCT_IDS[0],
        }
    }

    #[test]
    fn selects_by_serial_or_path() {
        let devices = [device("/dev/ttyUSB0", Some("ABC"))];
        assert_eq!(select_device(&devices, None).unwrap(), devices[0]);
        assert_eq!(select_device(&devices, Some("ABC")).unwrap(), devices[0]);
        assert_eq!(
            select_device(&devices, Some("/dev/ttyUSB0")).unwrap(),
            devices[0]
        );
    }

    #[test]
    fn rejects_ambiguous_auto_selection() {
        let devices = [device("/dev/ttyUSB0", None), device("/dev/ttyUSB1", None)];
        assert!(matches!(
            select_device(&devices, None),
            Err(Error::AmbiguousDevice)
        ));
    }
}
