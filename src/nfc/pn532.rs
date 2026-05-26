use std::path::PathBuf;

use async_trait::async_trait;
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use super::{CardPresence, NfcReader, ReaderCapabilities, ReaderError, ReaderFactory};

const PREAMBLE: [u8; 3] = [0x00, 0x00, 0xff];
const POSTAMBLE: u8 = 0x00;
const HOST_TO_PN532: u8 = 0xd4;
#[allow(dead_code)]
const PN532_TO_HOST: u8 = 0xd5;

#[derive(Debug, Clone)]
pub struct Pn532UartConfig {
    pub port: PathBuf,
    pub baud_rate: u32,
}

pub struct Pn532UartFactory {
    config: Pn532UartConfig,
}

impl Pn532UartFactory {
    pub fn new(config: Pn532UartConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ReaderFactory for Pn532UartFactory {
    fn backend_name(&self) -> &'static str {
        "pn532-uart"
    }

    async fn open(&self) -> Result<Box<dyn NfcReader>, ReaderError> {
        if self.config.port.as_os_str().is_empty() {
            return Err(ReaderError::Configuration(
                "PN532 UART port path must not be empty".to_string(),
            ));
        }

        let port_name = self.config.port.to_string_lossy().into_owned();
        let serial = tokio_serial::new(port_name.clone(), self.config.baud_rate)
            .open_native_async()
            .map_err(|error| {
                ReaderError::Transport(format!(
                    "failed to open PN532 UART port {port_name}: {error}"
                ))
            })?;

        Ok(Box::new(Pn532UartReader {
            config: self.config.clone(),
            #[allow(dead_code)]
            serial,
        }))
    }
}

pub struct Pn532UartReader {
    config: Pn532UartConfig,
    #[allow(dead_code)]
    serial: SerialStream,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pn532Frame {
    command: u8,
    data: Vec<u8>,
}

impl Pn532UartReader {
    fn build_host_frame(command: u8, data: &[u8]) -> Vec<u8> {
        let len = (2 + data.len()) as u8;
        let lcs = (!len).wrapping_add(1);

        let mut frame = Vec::with_capacity(8 + data.len());
        frame.extend_from_slice(&PREAMBLE);
        frame.push(len);
        frame.push(lcs);
        frame.push(HOST_TO_PN532);
        frame.push(command);
        frame.extend_from_slice(data);

        let checksum_source = &frame[5..];
        let checksum = checksum_source
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        let dcs = (!checksum).wrapping_add(1);
        frame.push(dcs);
        frame.push(POSTAMBLE);
        frame
    }

    #[allow(dead_code)]
    fn parse_device_frame(frame: &[u8]) -> Result<Pn532Frame, ReaderError> {
        if frame.len() < 8 {
            return Err(ReaderError::Protocol(format!(
                "PN532 frame too short: {} bytes",
                frame.len()
            )));
        }

        if frame[0..3] != PREAMBLE {
            return Err(ReaderError::Protocol(
                "PN532 frame preamble is invalid".to_string(),
            ));
        }

        let len = frame[3];
        let lcs = frame[4];
        if len.wrapping_add(lcs) != 0 {
            return Err(ReaderError::Protocol(
                "PN532 LEN/LCS checksum mismatch".to_string(),
            ));
        }

        let expected_len = len as usize + 7;
        if frame.len() != expected_len {
            return Err(ReaderError::Protocol(format!(
                "PN532 frame length mismatch: expected {expected_len} bytes, got {}",
                frame.len()
            )));
        }

        let payload = &frame[5..(5 + len as usize)];
        let dcs = frame[5 + len as usize];
        let payload_checksum = payload
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte))
            .wrapping_add(dcs);
        if payload_checksum != 0 {
            return Err(ReaderError::Protocol(
                "PN532 payload checksum mismatch".to_string(),
            ));
        }

        if frame[expected_len - 1] != POSTAMBLE {
            return Err(ReaderError::Protocol(
                "PN532 frame postamble is invalid".to_string(),
            ));
        }

        if payload.len() < 2 {
            return Err(ReaderError::Protocol(
                "PN532 payload is too short".to_string(),
            ));
        }

        if payload[0] != PN532_TO_HOST {
            return Err(ReaderError::Protocol(format!(
                "unexpected PN532 direction byte: 0x{:02x}",
                payload[0]
            )));
        }

        Ok(Pn532Frame {
            command: payload[1],
            data: payload[2..].to_vec(),
        })
    }

    #[allow(dead_code)]
    fn build_in_list_passive_target_frame() -> Vec<u8> {
        Self::build_host_frame(0x4a, &[0x01, 0x00])
    }
}

#[async_trait]
impl NfcReader for Pn532UartReader {
    fn capabilities(&self) -> ReaderCapabilities {
        ReaderCapabilities {
            name: "pn532-uart",
            supports_iso_dep: true,
            supports_apdu_exchange: true,
        }
    }

    async fn poll_card(&mut self) -> Result<Option<CardPresence>, ReaderError> {
        Err(ReaderError::Unsupported(format!(
            "PN532 polling over UART is not implemented yet for {} at {} baud",
            self.config.port.display(),
            self.config.baud_rate
        )))
    }

    async fn power_off(&mut self) -> Result<(), ReaderError> {
        Ok(())
    }

    async fn exchange_apdu(&mut self, _apdu: &[u8]) -> Result<Vec<u8>, ReaderError> {
        Err(ReaderError::Unsupported(
            "PN532 APDU exchange over UART is not implemented yet".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{HOST_TO_PN532, Pn532Frame, Pn532UartReader};

    #[test]
    fn build_host_frame_has_valid_preamble_and_checksum() {
        let frame = Pn532UartReader::build_host_frame(0x4a, &[0x01, 0x00]);

        assert_eq!(&frame[0..3], &[0x00, 0x00, 0xff]);
        assert_eq!(frame[5], HOST_TO_PN532);
        assert_eq!(frame.last().copied(), Some(0x00));
    }

    #[test]
    fn parse_device_frame_extracts_command_and_payload() {
        let data = [0x01, 0x01, 0x04, 0xde, 0xad, 0xbe, 0xef];
        let payload_len = (2 + data.len()) as u8;
        let lcs = (!payload_len).wrapping_add(1);

        let mut frame = vec![0x00, 0x00, 0xff, payload_len, lcs, 0xd5, 0x4b];
        frame.extend_from_slice(&data);

        let dcs = frame[5..]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        frame.push((!dcs).wrapping_add(1));
        frame.push(0x00);

        let decoded = Pn532UartReader::parse_device_frame(&frame).expect("frame should parse");
        assert_eq!(
            decoded,
            Pn532Frame {
                command: 0x4b,
                data: data.to_vec(),
            }
        );
    }
}
