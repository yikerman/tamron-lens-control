use std::{
    fmt,
    io::{self, Read, Write},
    time::{Duration, Instant},
};

use crc::{CRC_16_XMODEM, Crc};

use crate::{DeviceInfo, Error, Result};

const COMMAND_TIMEOUT: Duration = Duration::from_millis(500);
const COMMAND_CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_XMODEM);
const DESTINATION: u8 = 0;

const OP_READ_MEMORY: u8 = 0xf4;
const OP_WRITE_MEMORY: u8 = 0xf5;
const OP_CONNECT: u8 = 0xf8;
const OP_DISCONNECT: u8 = 0xf9;
const OP_COMMUNICATION_ERROR: u8 = 0xff;

pub(crate) trait Port: Read + Write + Send {
    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()>;
}

struct SerialPortIo(Box<dyn serialport::SerialPort>);

impl Read for SerialPortIo {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Write for SerialPortIo {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Port for SerialPortIo {
    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        self.0.set_timeout(timeout).map_err(io::Error::other)
    }
}

pub(crate) trait LensIo: Send {
    fn connect(&mut self) -> Result<u8>;
    fn read_block(&mut self, region: u8, block: u8) -> Result<[u8; 256]>;
    fn write_byte(&mut self, block: u8, offset: u8, value: u8) -> Result<()>;
    fn write_word(&mut self, block: u8, offset: u8, value: u16) -> Result<()>;
    fn restore_settings(&mut self, settings: &[u8; 512]) -> Result<()>;
    fn factory_reset(&mut self) -> Result<()>;
    fn disconnect(&mut self) -> Result<()>;
}

pub(crate) fn open(device: &DeviceInfo) -> Result<Box<dyn LensIo>> {
    log::debug!(target: "tlc", "opening {} at 19200 baud", device.port_name);
    let port = serialport::new(&device.port_name, 19_200)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(COMMAND_TIMEOUT)
        .open()
        .map_err(|source| Error::OpenPort {
            port: device.port_name.clone(),
            source,
        })?;
    Ok(Box::new(CommandSession::new(SerialPortIo(port))))
}

struct Response {
    data: Vec<u8>,
}

pub(crate) struct CommandSession<P> {
    port: P,
    sequence: u8,
}

impl<P: Port> CommandSession<P> {
    fn new(port: P) -> Self {
        Self { port, sequence: 0 }
    }

    fn next_sequence(&mut self) -> u8 {
        self.sequence = self.sequence.wrapping_add(1);
        if self.sequence == 0 {
            self.sequence = 1;
        }
        self.sequence
    }

    fn request(&mut self, op: u8, data: &[u8]) -> Result<Response> {
        let sequence = self.next_sequence();
        log::debug!(
            target: "tlc",
            "command {sequence:02X}: {}",
            describe_command(op, data)
        );
        let frame = encode_frame(sequence, op, data)?;
        log::trace!(target: "tlc", "TX {}", HexBytes(&frame));
        self.port.set_read_timeout(COMMAND_TIMEOUT)?;
        self.port.write_all(&frame)?;
        self.port.flush()?;

        let deadline = Instant::now() + COMMAND_TIMEOUT;
        loop {
            let response = self.read_frame(deadline)?;
            if response.sequence != sequence {
                continue;
            }
            if response.op != op {
                return Err(Error::InvalidResponse(format!(
                    "expected operation 0x{op:02X}, got 0x{:02X}",
                    response.op
                )));
            }
            if response.destination != DESTINATION {
                return Err(Error::InvalidResponse(format!(
                    "expected destination 0x00, got 0x{:02X}",
                    response.destination
                )));
            }
            if response.end != 0xf0 {
                return Err(Error::InvalidResponse(format!(
                    "expected end byte 0xF0, got 0x{:02X}",
                    response.end
                )));
            }
            return Ok(Response {
                data: response.data,
            });
        }
    }

    fn read_frame(&mut self, deadline: Instant) -> Result<DecodedFrame> {
        let mut header = [0_u8; 6];
        read_exact_until(&mut self.port, &mut header, deadline)?;
        if header[0] != 0x0f {
            log::trace!(target: "tlc", "RX {}", HexBytes(&header));
            return Err(Error::InvalidResponse(format!(
                "expected start byte 0x0F, got 0x{:02X}",
                header[0]
            )));
        }

        let length = usize::from(u16::from_le_bytes([header[4], header[5]]));
        if length == 0 {
            return Err(Error::InvalidResponse(
                "frame length excludes operation".into(),
            ));
        }
        let mut tail = vec![0_u8; length + 3];
        read_exact_until(&mut self.port, &mut tail, deadline)?;
        let mut raw_frame = Vec::with_capacity(header.len() + tail.len());
        raw_frame.extend_from_slice(&header);
        raw_frame.extend_from_slice(&tail);
        log::trace!(target: "tlc", "RX {}", HexBytes(&raw_frame));

        let mut crc_input = Vec::with_capacity(6 + length);
        crc_input.extend_from_slice(&header);
        crc_input.extend_from_slice(&tail[..length]);
        let expected_crc = COMMAND_CRC.checksum(&crc_input);
        let received_crc = u16::from_le_bytes([tail[length], tail[length + 1]]);
        if received_crc != expected_crc {
            return Err(Error::InvalidResponse(format!(
                "CRC mismatch: expected 0x{expected_crc:04X}, got 0x{received_crc:04X}"
            )));
        }

        let op = tail[0];
        if op == OP_COMMUNICATION_ERROR {
            return Err(Error::CommunicationError);
        }

        Ok(DecodedFrame {
            sequence: header[1],
            destination: header[2],
            op,
            data: tail[1..length].to_vec(),
            end: tail[length + 2],
        })
    }

    fn check_result(data: &[u8]) -> Result<()> {
        let result = *data
            .first()
            .ok_or_else(|| Error::InvalidResponse("response has no result byte".into()))?;
        if result < 0x80 {
            Ok(())
        } else {
            Err(Error::OperationRejected {
                code: result,
                message: if result == 0x83 {
                    "a camera may be attached or the operation is unsupported"
                } else {
                    "device reported failure"
                },
            })
        }
    }
}

impl<P: Port> LensIo for CommandSession<P> {
    fn connect(&mut self) -> Result<u8> {
        let response = self.request(OP_CONNECT, &[0])?;
        response
            .data
            .first()
            .copied()
            .ok_or_else(|| Error::InvalidResponse("connect response has no state byte".into()))
    }

    fn read_block(&mut self, region: u8, block: u8) -> Result<[u8; 256]> {
        let response = self.request(OP_READ_MEMORY, &[0, region, block, 0, 0])?;
        Self::check_result(&response.data)?;
        let payload = response.data.get(5..261).ok_or_else(|| {
            Error::InvalidResponse("memory response is shorter than 256 bytes".into())
        })?;
        payload
            .try_into()
            .map_err(|_| Error::InvalidResponse("invalid memory response length".into()))
    }

    fn write_byte(&mut self, block: u8, offset: u8, value: u8) -> Result<()> {
        let response = self.request(OP_WRITE_MEMORY, &[0, 1, block, offset, 1, value])?;
        Self::check_result(&response.data)
    }

    fn write_word(&mut self, block: u8, offset: u8, value: u16) -> Result<()> {
        let [low, high] = value.to_le_bytes();
        let response = self.request(OP_WRITE_MEMORY, &[0, 1, block, offset, 2, low, high])?;
        Self::check_result(&response.data)
    }

    fn restore_settings(&mut self, settings: &[u8; 512]) -> Result<()> {
        let mut first = Vec::with_capacity(517);
        first.extend_from_slice(&[0, 1, 0, 0, 0]);
        first.extend_from_slice(settings);
        let response = self.request(OP_WRITE_MEMORY, &first)?;
        Self::check_result(&response.data)?;

        let mut second = Vec::with_capacity(261);
        second.extend_from_slice(&[0, 1, 1, 0, 0]);
        second.extend_from_slice(&settings[256..]);
        let response = self.request(OP_WRITE_MEMORY, &second)?;
        Self::check_result(&response.data)
    }

    fn factory_reset(&mut self) -> Result<()> {
        let response = self.request(OP_WRITE_MEMORY, &[1, 1, 0, 0, 0])?;
        Self::check_result(&response.data)
    }

    fn disconnect(&mut self) -> Result<()> {
        self.request(OP_DISCONNECT, &[]).map(|_| ())
    }
}

struct DecodedFrame {
    sequence: u8,
    destination: u8,
    op: u8,
    data: Vec<u8>,
    end: u8,
}

fn encode_frame(sequence: u8, op: u8, data: &[u8]) -> Result<Vec<u8>> {
    let length = u16::try_from(data.len() + 1)
        .map_err(|_| Error::InvalidValue("command payload exceeds protocol length".into()))?;
    let mut frame = Vec::with_capacity(data.len() + 10);
    frame.extend_from_slice(&[0x0f, sequence, DESTINATION, 0]);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.push(op);
    frame.extend_from_slice(data);
    frame.extend_from_slice(&COMMAND_CRC.checksum(&frame).to_le_bytes());
    frame.push(0xf0);
    Ok(frame)
}

fn describe_command(op: u8, data: &[u8]) -> String {
    match (op, data) {
        (OP_CONNECT, _) => "connect".into(),
        (OP_DISCONNECT, _) => "disconnect".into(),
        (OP_READ_MEMORY, [0, 0, block, offset, size, ..]) => {
            describe_read("descriptor", *block, *offset, *size)
        }
        (OP_READ_MEMORY, [0, 1, block, offset, size, ..]) => {
            describe_read("settings", *block, *offset, *size)
        }
        (OP_WRITE_MEMORY, [1, 1, 0, 0, 0]) => "restore factory settings".into(),
        (OP_WRITE_MEMORY, [0, 1, block, offset, 0, payload @ ..]) => format!(
            "restore settings block {block} at offset {offset} ({} payload bytes)",
            payload.len()
        ),
        (OP_WRITE_MEMORY, [0, 1, block, offset, size, _payload @ ..]) => format!(
            "write settings block {block} at offset {offset} ({size} {})",
            if *size == 1 { "byte" } else { "bytes" }
        ),
        _ => format!("operation 0x{op:02X}"),
    }
}

fn describe_read(region: &str, block: u8, offset: u8, size: u8) -> String {
    if size == 0 {
        format!("read {region} block {block}")
    } else {
        format!("read {region} block {block} at offset {offset} ({size} bytes)")
    }
}

struct HexBytes<'a>(&'a [u8]);

impl fmt::Display for HexBytes<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str(" ")?;
            }
            write!(formatter, "{byte:02X}")?;
        }
        Ok(())
    }
}

fn read_exact_until(port: &mut impl Port, buffer: &mut [u8], deadline: Instant) -> Result<()> {
    let mut read = 0;
    while read < buffer.len() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(Error::Timeout)?;
        if remaining.is_zero() {
            return Err(Error::Timeout);
        }
        port.set_read_timeout(remaining)?;
        match port.read(&mut buffer[read..]) {
            Ok(0) => {
                return Err(Error::InvalidResponse(
                    "serial port reached end of stream".into(),
                ));
            }
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => return Err(Error::Timeout),
            Err(error) => return Err(Error::Io(error)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct FakePort {
        reads: VecDeque<u8>,
        writes: Vec<u8>,
    }

    impl Read for FakePort {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.reads.is_empty() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let count = buffer.len().min(3).min(self.reads.len());
            for byte in buffer.iter_mut().take(count) {
                *byte = self.reads.pop_front().unwrap();
            }
            Ok(count)
        }
    }

    impl Write for FakePort {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.writes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Port for FakePort {
        fn set_read_timeout(&mut self, _timeout: Duration) -> io::Result<()> {
            Ok(())
        }
    }

    fn response(sequence: u8, op: u8, data: &[u8]) -> Vec<u8> {
        encode_frame(sequence, op, data).unwrap()
    }

    #[test]
    fn xmodem_crc_matches_known_vector() {
        assert_eq!(COMMAND_CRC.checksum(b"123456789"), 0x31c3);
    }

    #[test]
    fn request_encodes_and_accepts_partial_reads() {
        let reply = response(1, OP_CONNECT, &[1]);
        let port = FakePort {
            reads: reply.into(),
            writes: Vec::new(),
        };
        let mut session = CommandSession::new(port);
        assert_eq!(session.connect().unwrap(), 1);
        assert_eq!(
            session.port.writes,
            encode_frame(1, OP_CONNECT, &[0]).unwrap()
        );
    }

    #[test]
    fn sequence_mismatch_is_discarded() {
        let replies = [response(2, OP_CONNECT, &[1]), response(1, OP_CONNECT, &[2])].concat();
        let port = FakePort {
            reads: replies.into(),
            writes: Vec::new(),
        };
        let mut session = CommandSession::new(port);
        assert_eq!(session.connect().unwrap(), 2);
    }

    #[test]
    fn communication_error_skips_footer_checks() {
        let mut reply = response(9, OP_COMMUNICATION_ERROR, &[]);
        *reply.last_mut().unwrap() = 0;
        let crc_index = reply.len() - 3;
        let crc = COMMAND_CRC.checksum(&reply[..crc_index]);
        reply[crc_index..crc_index + 2].copy_from_slice(&crc.to_le_bytes());
        let port = FakePort {
            reads: reply.into(),
            writes: Vec::new(),
        };
        let mut session = CommandSession::new(port);
        assert!(matches!(session.connect(), Err(Error::CommunicationError)));
    }

    #[test]
    fn sequence_wrap_skips_zero() {
        let port = FakePort {
            reads: VecDeque::new(),
            writes: Vec::new(),
        };
        let mut session = CommandSession::new(port);
        session.sequence = 0xfe;
        assert_eq!(session.next_sequence(), 0xff);
        assert_eq!(session.next_sequence(), 1);
    }

    #[test]
    fn restore_emits_the_two_asymmetric_frames() {
        let replies = [
            response(1, OP_WRITE_MEMORY, &[0]),
            response(2, OP_WRITE_MEMORY, &[0]),
        ]
        .concat();
        let port = FakePort {
            reads: replies.into(),
            writes: Vec::new(),
        };
        let mut session = CommandSession::new(port);
        let settings = [0x5a; 512];
        session.restore_settings(&settings).unwrap();

        let mut first_data = vec![0, 1, 0, 0, 0];
        first_data.extend_from_slice(&settings);
        let mut second_data = vec![0, 1, 1, 0, 0];
        second_data.extend_from_slice(&settings[256..]);
        let expected = [
            encode_frame(1, OP_WRITE_MEMORY, &first_data).unwrap(),
            encode_frame(2, OP_WRITE_MEMORY, &second_data).unwrap(),
        ]
        .concat();
        assert_eq!(session.port.writes, expected);
    }

    #[test]
    fn timeout_does_not_retry_request() {
        let port = FakePort {
            reads: VecDeque::new(),
            writes: Vec::new(),
        };
        let mut session = CommandSession::new(port);
        assert!(matches!(session.connect(), Err(Error::Timeout)));
        assert_eq!(
            session.port.writes,
            encode_frame(1, OP_CONNECT, &[0]).unwrap()
        );
    }

    #[test]
    fn rejected_result_is_preserved() {
        let reply = response(1, OP_WRITE_MEMORY, &[0x83]);
        let port = FakePort {
            reads: reply.into(),
            writes: Vec::new(),
        };
        let mut session = CommandSession::new(port);
        assert!(matches!(
            session.write_byte(0, 1, 2),
            Err(Error::OperationRejected { code: 0x83, .. })
        ));
    }

    #[test]
    fn command_descriptions_are_readable() {
        assert_eq!(describe_command(OP_CONNECT, &[0]), "connect");
        assert_eq!(
            describe_command(OP_READ_MEMORY, &[0, 0, 0, 0, 0]),
            "read descriptor block 0"
        );
        assert_eq!(
            describe_command(OP_WRITE_MEMORY, &[0, 1, 0, 9, 1, 2]),
            "write settings block 0 at offset 9 (1 byte)"
        );
        assert_eq!(format!("{}", HexBytes(&[0x0f, 0xa5, 0])), "0F A5 00");
    }
}
