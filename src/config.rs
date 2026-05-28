use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Result, bail};
use clap::{Parser, ValueEnum};

use crate::nfc::pn532::Pn532UartConfig;

#[derive(Debug, Clone, Parser)]
#[command(
    author,
    version,
    about = "USB/IP virtual CCID exporter backed by NFC readers"
)]
pub struct Cli {
    #[arg(long, default_value = "0.0.0.0:3240")]
    pub listen_addr: SocketAddr,

    #[arg(long, value_enum, default_value_t = ReaderBackend::Pn532Uart)]
    pub backend: ReaderBackend,

    #[arg(long, default_value = "")]
    pub serial_port: String,

    #[arg(long, default_value_t = 115_200)]
    pub serial_baud_rate: u32,

    #[arg(long, default_value_t = 500)]
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum ReaderBackend {
    Pn532Uart,
    Dummy,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub listen_addr: SocketAddr,
    pub reader: ReaderConfig,
    pub poll_interval: Duration,
}

#[derive(Debug, Clone)]
pub enum ReaderConfig {
    Pn532Uart(Pn532UartConfig),
    Dummy,
}

impl TryFrom<Cli> for AppConfig {
    type Error = anyhow::Error;

    fn try_from(cli: Cli) -> Result<Self> {
        let reader = match cli.backend {
            ReaderBackend::Pn532Uart => {
                if cli.serial_port.trim().is_empty() {
                    bail!("--serial-port must be provided for the pn532-uart backend");
                }

                ReaderConfig::Pn532Uart(Pn532UartConfig {
                    port: PathBuf::from(cli.serial_port),
                    baud_rate: cli.serial_baud_rate,
                })
            }
            ReaderBackend::Dummy => ReaderConfig::Dummy,
        };

        Ok(Self {
            listen_addr: cli.listen_addr,
            reader,
            poll_interval: Duration::from_millis(cli.poll_interval_ms),
        })
    }
}
