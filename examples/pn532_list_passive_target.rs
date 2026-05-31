use std::{env, io::Read, process::ExitCode, thread::sleep, time::Duration};

use anyhow::{bail, Context, Result};
use pn532::{
    requests::{BorrowedRequest, Command, SAMMode},
    serialport::{SerialPortInterface, SysTimer},
    Error as Pn532Error, IntoDuration, Pn532, Request,
};

const BAUD_RATE: u32 = 115_200;
const INIT_TIMEOUT_MS: u64 = 1_000;
const POLL_TIMEOUT_MS: u64 = 3_000;
const APDU_TIMEOUT_MS: u64 = 1_000;
const TOTAL_POLL_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_PORT: &str = "COM6";
const CTAP_AID: &[u8] = &[0xA0, 0x00, 0x00, 0x06, 0x47, 0x2F, 0x00, 0x01];
const SELECT_CTAP_AID_APDU: &[u8] = &[
    0x00, 0xA4, 0x04, 0x00, 0x08, 0xA0, 0x00, 0x00, 0x06, 0x47, 0x2F, 0x00, 0x01, 0x00,
];
const RF_MAX_RETRIES: &[u8] = &[0x05, 0x00, 0x00, 0x02];
const PREAMBLE: [u8; 3] = [0x00, 0x00, 0xFF];
const PN532_TO_HOST: u8 = 0xD5;
const POSTAMBLE: u8 = 0x00;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let port_name = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_PORT.to_string());

    let serial_port = serialport::new(&port_name, BAUD_RATE)
        .timeout(Duration::from_millis(INIT_TIMEOUT_MS))
        .open()
        .with_context(|| format!("failed to open PN532 UART port {port_name}"))?;

    let mut interface = SerialPortInterface { port: serial_port };
    interface
        .send_wakeup_message()
        .context("failed to send PN532 wakeup message")?;

    let mut pn532 = Pn532::<_, _, 320>::new(interface, SysTimer::new());
    pn532
        .process(
            &Request::sam_configuration(SAMMode::Normal, false),
            0,
            INIT_TIMEOUT_MS.ms(),
        )
        .map_err(|error| anyhow::anyhow!("failed to configure PN532 SAM: {error:?}"))?;

    pn532
        .process(
            BorrowedRequest::new(Command::RFConfiguration, RF_MAX_RETRIES),
            0,
            INIT_TIMEOUT_MS.ms(),
        )
        .map_err(|error| anyhow::anyhow!("failed to configure PN532 RF retries: {error:?}"))?;

    let response = poll_iso_a_target(&mut pn532, &port_name)?;

    let Some(target) = print_iso_type_a_target(&response)? else {
        return Ok(());
    };

    println!("Selecting CTAP AID: {}", hex(CTAP_AID));
    println!("APDU command: {}", hex(SELECT_CTAP_AID_APDU));

    let response = exchange_apdu(&mut pn532, target, SELECT_CTAP_AID_APDU)
        .context("failed to select CTAP AID")?;
    println!("APDU response: {}", hex(&response));
    print_apdu_status(&response)
}

fn poll_iso_a_target(
    pn532: &mut Pn532<SerialPortInterface, SysTimer, 320>,
    port_name: &str,
) -> Result<Vec<u8>> {
    let attempts = TOTAL_POLL_TIMEOUT_MS.div_ceil(POLL_TIMEOUT_MS);

    for attempt in 1..=attempts {
        match process_dynamic_response(
            pn532,
            (&Request::INLIST_ONE_ISO_A_TARGET).into(),
            POLL_TIMEOUT_MS,
        ) {
            Ok(response) if response.first().copied().unwrap_or(0) != 0 => return Ok(response),
            Ok(_) => println!("No ISO14443-A target yet; retrying ({attempt}/{attempts})"),
            Err(Pn532Error::TimeoutResponse) => {
                println!("No target response yet; retrying ({attempt}/{attempts})");
            }
            Err(error) => bail!("failed to run InListPassiveTarget: {error:?}"),
        }
    }

    bail!("timed out waiting for an ISO14443-A target on {port_name}")
}

fn print_iso_type_a_target(response: &[u8]) -> Result<Option<u8>> {
    if response.is_empty() || response[0] == 0 {
        println!("No ISO14443-A target found");
        return Ok(None);
    }

    if response.len() < 6 {
        bail!(
            "InListPassiveTarget response too short: {} bytes",
            response.len()
        );
    }

    let target_count = response[0];
    let target_number = response[1];
    let sens_res = &response[2..4];
    let sel_res = response[4];
    let uid_len = response[5] as usize;
    let uid_start = 6;
    let uid_end = uid_start + uid_len;

    if response.len() < uid_end {
        bail!(
            "InListPassiveTarget response ended before UID: {} bytes",
            response.len()
        );
    }

    println!("Targets: {target_count}");
    println!("Target number: {target_number}");
    println!("SENS_RES: {}", hex(sens_res));
    println!("SEL_RES: {sel_res:02X}");
    println!("UID: {}", hex(&response[uid_start..uid_end]));

    if response.len() > uid_end {
        let ats_len = response[uid_end] as usize;
        let ats_start = uid_end + 1;
        let ats_end = ats_start + ats_len;
        if response.len() >= ats_end {
            println!(
                "ISO-DEP activation completed; ATS: {}",
                hex(&response[ats_start..ats_end])
            );
            if response.len() > ats_end {
                println!("Target extra: {}", hex(&response[ats_end..]));
            }
        } else {
            println!("ATS/extra: {}", hex(&response[uid_end..]));
        }
    }

    Ok(Some(target_number))
}

fn exchange_apdu(
    pn532: &mut Pn532<SerialPortInterface, SysTimer, 320>,
    target: u8,
    apdu: &[u8],
) -> Result<Vec<u8>> {
    let mut data = Vec::with_capacity(apdu.len() + 1);
    data.push(target);
    data.extend_from_slice(apdu);

    let request = BorrowedRequest::new(Command::InDataExchange, &data);
    let response = process_dynamic_response(pn532, request, APDU_TIMEOUT_MS)
        .map_err(|error| anyhow::anyhow!("PN532 InDataExchange failed: {error:?}"))?;

    if response.is_empty() {
        bail!("PN532 returned an empty InDataExchange response");
    }

    if response[0] != 0x00 {
        bail!("PN532 InDataExchange status 0x{:02X}", response[0]);
    }

    Ok(response[1..].to_vec())
}

fn print_apdu_status(response: &[u8]) -> Result<()> {
    if response.len() < 2 {
        bail!(
            "APDU response too short for SW1/SW2: {} bytes",
            response.len()
        );
    }

    let sw1 = response[response.len() - 2];
    let sw2 = response[response.len() - 1];
    let payload = &response[..response.len() - 2];

    if !payload.is_empty() {
        println!("APDU data: {}", hex(payload));
    }
    println!("SW1/SW2: {sw1:02X} {sw2:02X}");

    if (sw1, sw2) == (0x90, 0x00) {
        println!("CTAP AID selected successfully");
    } else {
        bail!("CTAP AID select failed with SW1/SW2 {sw1:02X} {sw2:02X}");
    }

    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn process_dynamic_response(
    pn532: &mut Pn532<SerialPortInterface, SysTimer, 320>,
    request: BorrowedRequest<'_>,
    timeout_ms: u64,
) -> Result<Vec<u8>, Pn532Error<std::io::Error>> {
    let expected_response_command = request.command as u8 + 1;

    pn532.send(request)?;
    wait_serial_ready(pn532, timeout_ms).map_err(|_| Pn532Error::TimeoutAck)?;
    pn532.receive_ack()?;
    wait_serial_ready(pn532, timeout_ms).map_err(|_| Pn532Error::TimeoutResponse)?;
    receive_dynamic_response(pn532, expected_response_command)
}

fn wait_serial_ready(
    pn532: &mut Pn532<SerialPortInterface, SysTimer, 320>,
    timeout_ms: u64,
) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if pn532.interface.port.bytes_to_read()? > 0 {
            return Ok(());
        }
        sleep(Duration::from_millis(5));
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "timed out waiting for PN532 serial data",
    ))
}

fn receive_dynamic_response(
    pn532: &mut Pn532<SerialPortInterface, SysTimer, 320>,
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
