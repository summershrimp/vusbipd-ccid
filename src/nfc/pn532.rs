use std::{
    io::Read,
    path::PathBuf,
    thread::sleep,
    time::{Duration, Instant},
};

use pn532::{
    requests::{BorrowedRequest, Command, SAMMode},
    serialport::{SerialPortInterface, SysTimer},
    Error as Pn532Error, IntoDuration, Pn532, Request,
};
use tracing::debug;

use super::{
    CardPresence, CardProtocol, NfcReader, ReaderCapabilities, ReaderError, ReaderFactory,
};

const PN532_BUFFER_SIZE: usize = 320;
const PN532_INIT_TIMEOUT_MS: u64 = 1_000;
const PN532_POLL_TIMEOUT_MS: u64 = 500;
const PN532_APDU_TIMEOUT_MS: u64 = 1_000;
const PN532_POLL_INTERVAL_MS: u64 = 1_000;
const PN532_ABSENT_POLL_INTERVAL_MS: u64 = 2_000;
const ISO_A_TARGET_SLOT: u8 = 0x01;
const RF_MAX_RETRIES: &[u8] = &[0x05, 0x00, 0x00, 0x00];
const RF_FIELD_ON: &[u8] = &[0x01, 0x01];
const RF_FIELD_OFF: &[u8] = &[0x01, 0x00];
const PREAMBLE: [u8; 3] = [0x00, 0x00, 0xff];
const PN532_TO_HOST: u8 = 0xd5;
const POSTAMBLE: u8 = 0x00;

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
            .timeout(Duration::from_millis(PN532_APDU_TIMEOUT_MS))
            .open()
            .map_err(|error| {
                ReaderError::Transport(format!(
                    "failed to open PN532 UART port {port_name}: {error}"
                ))
            })?;

        let mut interface = SerialPortInterface { port: serial_port };
        interface.send_wakeup_message().map_err(|error| {
            ReaderError::Transport(format!("failed to wake PN532 on {port_name}: {error}"))
        })?;

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

        pn532
            .process(
                BorrowedRequest::new(Command::RFConfiguration, RF_MAX_RETRIES),
                0,
                PN532_INIT_TIMEOUT_MS.ms(),
            )
            .map_err(|error| map_pn532_error("configure RF retries", error))?;

        Ok(Box::new(Pn532UartReader {
            config: self.config.clone(),
            pn532,
            active_target: None,
            cached_card: None,
            last_poll: None,
            rf_field_enabled: false,
            firmware_version,
        }))
    }
}

pub struct Pn532UartReader {
    config: Pn532UartConfig,
    pn532: Pn532Device,
    active_target: Option<u8>,
    cached_card: Option<CardPresence>,
    last_poll: Option<Instant>,
    rf_field_enabled: bool,
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
        let now = Instant::now();
        let poll_interval = if self.cached_card.is_some() {
            Duration::from_millis(PN532_POLL_INTERVAL_MS)
        } else {
            Duration::from_millis(PN532_ABSENT_POLL_INTERVAL_MS)
        };

        if self
            .last_poll
            .is_some_and(|last_poll| now.duration_since(last_poll) < poll_interval)
        {
            debug!(
                cached_present = self.cached_card.is_some(),
                "returning cached PN532 card presence"
            );
            return Ok(self.cached_card.clone());
        }

        debug!("polling PN532 for ISO14443-A target");
        self.set_rf_field(true)?;
        self.last_poll = Some(now);

        let response = process_dynamic_response(
            &mut self.pn532,
            (&Request::INLIST_ONE_ISO_A_TARGET).into(),
            PN532_POLL_TIMEOUT_MS,
        )
        .map_err(|error| map_pn532_error("poll for ISO14443A target", error))?;

        if response.is_empty() || response[0] == 0 {
            self.active_target = None;
            self.cached_card = None;
            self.set_rf_field(false)?;
            debug!("PN532 poll found no ISO14443-A target");
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

        let card = CardPresence {
            uid: response[uid_start..uid_end].to_vec(),
            protocol: CardProtocol::IsoDep,
            historical_bytes,
        };
        debug!(
            target,
            uid = %format_hex(&card.uid),
            historical_bytes = %format_hex(&card.historical_bytes),
            "PN532 poll found ISO14443-A target"
        );
        self.cached_card = Some(card.clone());

        Ok(Some(card))
    }

    fn power_off(&mut self) -> Result<(), ReaderError> {
        self.active_target = None;
        self.set_rf_field(false)?;
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

        debug!(
            target,
            apdu = %format_hex(apdu),
            "exchanging APDU through PN532"
        );

        let mut request_data = Vec::with_capacity(apdu.len() + 1);
        request_data.push(target);
        request_data.extend_from_slice(apdu);

        let request = BorrowedRequest::new(Command::InDataExchange, &request_data);
        let response = process_dynamic_response(&mut self.pn532, request, PN532_APDU_TIMEOUT_MS)
            .map_err(|error| map_pn532_error("exchange APDU with NFC target", error))?;

        if response.is_empty() {
            return Err(ReaderError::Protocol(
                "PN532 returned an empty APDU exchange response".to_string(),
            ));
        }

        debug!(
            target,
            pn532_status = format_args!("0x{:02x}", response[0]),
            response = %format_hex(&response[1..]),
            "received APDU response through PN532"
        );

        if response[0] != 0x00 {
            return Err(ReaderError::Protocol(format!(
                "PN532 reported APDU exchange status 0x{:02x}",
                response[0]
            )));
        }

        Ok(response[1..].to_vec())
    }
}

fn format_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
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

fn process_dynamic_response(
    pn532: &mut Pn532Device,
    request: BorrowedRequest<'_>,
    timeout_ms: u64,
) -> Result<Vec<u8>, Pn532Error<std::io::Error>> {
    let expected_response_command = request.command as u8 + 1;

    pn532.send(request)?;
    if !wait_serial_ready(pn532, timeout_ms).map_err(Pn532Error::InterfaceError)? {
        return Err(Pn532Error::TimeoutAck);
    }
    pn532.receive_ack()?;
    if !wait_serial_ready(pn532, timeout_ms).map_err(Pn532Error::InterfaceError)? {
        return Err(Pn532Error::TimeoutResponse);
    }
    receive_dynamic_response(pn532, expected_response_command)
}

fn wait_serial_ready(pn532: &mut Pn532Device, timeout_ms: u64) -> std::io::Result<bool> {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if pn532.interface.port.bytes_to_read()? > 0 {
            return Ok(true);
        }
        sleep(Duration::from_millis(5));
    }

    Ok(false)
}

fn receive_dynamic_response(
    pn532: &mut Pn532Device,
    expected_response_command: u8,
) -> Result<Vec<u8>, Pn532Error<std::io::Error>> {
    let mut header = [0; 5];
    pn532.interface.port.read_exact(&mut header)?;

    if header[..3] != PREAMBLE {
        return Err(Pn532Error::BadResponseFrame);
    }

    let frame_len = header[3];
    if frame_len.wrapping_add(header[4]) != 0 {
        return Err(Pn532Error::CrcError);
    }
    if frame_len == 0 {
        return Err(Pn532Error::BadResponseFrame);
    }
    if frame_len == 1 {
        return Err(Pn532Error::Syntax);
    }

    let mut frame = vec![0; frame_len as usize + 2];
    pn532.interface.port.read_exact(&mut frame)?;

    if frame[frame.len() - 1] != POSTAMBLE {
        return Err(Pn532Error::BadResponseFrame);
    }
    if frame[0] != PN532_TO_HOST || frame[1] != expected_response_command {
        return Err(Pn532Error::BadResponseFrame);
    }

    let checksum = frame[..frame.len() - 1]
        .iter()
        .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    if checksum != 0 {
        return Err(Pn532Error::CrcError);
    }

    Ok(frame[2..frame.len() - 2].to_vec())
}

impl Pn532UartReader {
    fn set_rf_field(&mut self, enabled: bool) -> Result<(), ReaderError> {
        if self.rf_field_enabled == enabled {
            return Ok(());
        }

        let data = if enabled { RF_FIELD_ON } else { RF_FIELD_OFF };
        self.pn532
            .process(
                BorrowedRequest::new(Command::RFConfiguration, data),
                0,
                PN532_INIT_TIMEOUT_MS.ms(),
            )
            .map_err(|error| map_pn532_error("set PN532 RF field state", error))?;

        self.rf_field_enabled = enabled;
        Ok(())
    }

    pub fn firmware_version(&self) -> &[u8] {
        &self.firmware_version
    }

    pub fn port_path(&self) -> &PathBuf {
        &self.config.port
    }
}
