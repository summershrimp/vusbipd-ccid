use std::{env, process::ExitCode, time::Duration};

use anyhow::{bail, Context, Result};
use pn532::{
    requests::SAMMode,
    serialport::{SerialPortInterface, SysTimer},
    IntoDuration, Pn532, Request,
};

const BAUD_RATE: u32 = 115_200;
const INIT_TIMEOUT_MS: u64 = 1_000;
const POLL_TIMEOUT_MS: u64 = 1_000;
const RESPONSE_LEN: usize = 48;

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
        .context("usage: cargo run --example pn532_list_passive_target -- <serial-port>")?;

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

    let response = pn532
        .process(
            &Request::INLIST_ONE_ISO_A_TARGET,
            RESPONSE_LEN,
            POLL_TIMEOUT_MS.ms(),
        )
        .map_err(|error| anyhow::anyhow!("failed to run InListPassiveTarget: {error:?}"))?;

    print_iso_type_a_target(response)
}

fn print_iso_type_a_target(response: &[u8]) -> Result<()> {
    if response.is_empty() || response[0] == 0 {
        println!("No ISO14443-A target found");
        return Ok(());
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
        println!("ATS/extra: {}", hex(&response[uid_end..]));
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
