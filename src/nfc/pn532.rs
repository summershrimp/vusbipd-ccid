use std::{path::PathBuf, time::Duration};

use pn532::{
    Error as Pn532Error, IntoDuration, Pn532, Request,
    requests::{BorrowedRequest, Command, SAMMode},
    serialport::{SerialPortInterface, SysTimer},
};

use super::{
    CardPresence, CardProtocol, NfcReader, ReaderCapabilities, ReaderError, ReaderFactory,
};

const PN532_BUFFER_SIZE: usize = 320;
const PN532_INIT_TIMEOUT_MS: u64 = 100;
const PN532_POLL_TIMEOUT_MS: u64 = 500;
const PN532_APDU_TIMEOUT_MS: u64 = 1_000;
const PN532_POLL_RESPONSE_LEN: usize = 48;
const PN532_APDU_RESPONSE_LEN: usize = 280;
const ISO_A_TARGET_SLOT: u8 = 0x01;

type Pn532Device = Pn532<SerialPortInterface, SysTimer, PN532_BUFFER_SIZE>;

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

impl ReaderFactory for Pn532UartFactory {
    fn backend_name(&self) -> &'static str {
        "pn532-uart"
    }

    fn open(&self) -> Result<Box<dyn NfcReader>, ReaderError> {
        if self.config.port.as_os_str().is_empty() {
            return Err(ReaderError::Configuration(
                "PN532 UART port path must not be empty".to_string(),
            ));
        }

        let port_name = self.config.port.to_string_lossy().into_owned();
        let serial_port = serialport::new(&port_name, self.config.baud_rate)
            .timeout(Duration::from_millis(PN532_INIT_TIMEOUT_MS))
            .open()
            .map_err(|error| {
                ReaderError::Transport(format!(
                    "failed to open PN532 UART port {port_name}: {error}"
                ))
            })?;

        let interface = SerialPortInterface { port: serial_port };
        let mut pn532 = Pn532::new(interface, SysTimer::new());

        pn532
            .process(
                &Request::sam_configuration(SAMMode::Normal, false),
                0,
                PN532_INIT_TIMEOUT_MS.ms(),
            )
            .map_err(|error| map_pn532_error("initialize SAM configuration", error))?;

        let firmware_version = {
            let firmware = pn532
                .process(
                    &Request::GET_FIRMWARE_VERSION,
                    4,
                    PN532_INIT_TIMEOUT_MS.ms(),
                )
                .map_err(|error| map_pn532_error("read firmware version", error))?;
            firmware.to_vec()
        };

        Ok(Box::new(Pn532UartReader {
            config: self.config.clone(),
            pn532,
            active_target: None,
            firmware_version,
        }))
    }
}

pub struct Pn532UartReader {
    config: Pn532UartConfig,
    pn532: Pn532Device,
    active_target: Option<u8>,
    firmware_version: Vec<u8>,
}

impl NfcReader for Pn532UartReader {
    fn capabilities(&self) -> ReaderCapabilities {
        ReaderCapabilities {
            name: "pn532-uart",
            supports_iso_dep: true,
            supports_apdu_exchange: true,
        }
    }

    fn poll_card(&mut self) -> Result<Option<CardPresence>, ReaderError> {
        let response = self
            .pn532
            .process(
                &Request::INLIST_ONE_ISO_A_TARGET,
                PN532_POLL_RESPONSE_LEN,
                PN532_POLL_TIMEOUT_MS.ms(),
            )
            .map_err(|error| map_pn532_error("poll for ISO14443A target", error))?;

        if response.is_empty() || response[0] == 0 {
            self.active_target = None;
            return Ok(None);
        }

        if response.len() < 6 {
            return Err(ReaderError::Protocol(format!(
                "PN532 target response too short: {} bytes",
                response.len()
            )));
        }

        let target = response[1];
        let uid_len = response[5] as usize;
        let uid_start = 6;
        let uid_end = uid_start + uid_len;

        if response.len() < uid_end {
            return Err(ReaderError::Protocol(format!(
                "PN532 target response truncated before UID: {} bytes",
                response.len()
            )));
        }

        let historical_bytes = if response.len() > uid_end {
            let ats_len = response[uid_end] as usize;
            let ats_start = uid_end + 1;
            let ats_end = ats_start + ats_len;
            if response.len() >= ats_end {
                response[ats_start..ats_end].to_vec()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        self.active_target = Some(target);

        Ok(Some(CardPresence {
            uid: response[uid_start..uid_end].to_vec(),
            protocol: CardProtocol::IsoDep,
            historical_bytes,
        }))
    }

    fn power_off(&mut self) -> Result<(), ReaderError> {
        self.active_target = None;
        Ok(())
    }

    fn exchange_apdu(&mut self, apdu: &[u8]) -> Result<Vec<u8>, ReaderError> {
        if apdu.len() + 1 > PN532_BUFFER_SIZE - 9 {
            return Err(ReaderError::Unsupported(format!(
                "APDU of {} bytes exceeds current PN532 buffer budget",
                apdu.len()
            )));
        }

        let target = match self.active_target {
            Some(target) => target,
            None => match self.poll_card()? {
                Some(_) => self.active_target.unwrap_or(ISO_A_TARGET_SLOT),
                None => {
                    return Err(ReaderError::Protocol(
                        "cannot exchange APDU because no NFC target is present".to_string(),
                    ));
                }
            },
        };

        let mut request_data = Vec::with_capacity(apdu.len() + 1);
        request_data.push(target);
        request_data.extend_from_slice(apdu);

        let request = BorrowedRequest::new(Command::InDataExchange, &request_data);
        let response = self
            .pn532
            .process(request, PN532_APDU_RESPONSE_LEN, PN532_APDU_TIMEOUT_MS.ms())
            .map_err(|error| map_pn532_error("exchange APDU with NFC target", error))?;

        if response.is_empty() {
            return Err(ReaderError::Protocol(
                "PN532 returned an empty APDU exchange response".to_string(),
            ));
        }

        if response[0] != 0x00 {
            return Err(ReaderError::Protocol(format!(
                "PN532 reported APDU exchange status 0x{:02x}",
                response[0]
            )));
        }

        Ok(response[1..].to_vec())
    }
}

fn map_pn532_error(action: &str, error: Pn532Error<std::io::Error>) -> ReaderError {
    match error {
        Pn532Error::BadAck => ReaderError::Protocol(format!("failed to {action}: bad PN532 ACK")),
        Pn532Error::BadResponseFrame => ReaderError::Protocol(format!(
            "failed to {action}: malformed PN532 response frame"
        )),
        Pn532Error::Syntax => {
            ReaderError::Protocol(format!("failed to {action}: PN532 syntax error frame"))
        }
        Pn532Error::CrcError => {
            ReaderError::Protocol(format!("failed to {action}: PN532 checksum error"))
        }
        Pn532Error::BufTooSmall => ReaderError::Protocol(format!(
            "failed to {action}: configured PN532 response buffer is too small"
        )),
        Pn532Error::TimeoutAck => {
            ReaderError::Transport(format!("failed to {action}: timeout waiting for PN532 ACK"))
        }
        Pn532Error::TimeoutResponse => ReaderError::Transport(format!(
            "failed to {action}: timeout waiting for PN532 response"
        )),
        Pn532Error::InterfaceError(inner) => ReaderError::Io(inner.to_string()),
    }
}

impl Pn532UartReader {
    pub fn firmware_version(&self) -> &[u8] {
        &self.firmware_version
    }

    pub fn port_path(&self) -> &PathBuf {
        &self.config.port
    }
}
