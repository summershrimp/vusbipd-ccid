use async_trait::async_trait;
use thiserror::Error;

pub mod pn532;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardProtocol {
    Iso14443A,
    Iso14443B,
    IsoDep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardPresence {
    pub uid: Vec<u8>,
    pub protocol: CardProtocol,
    pub historical_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderCapabilities {
    pub name: &'static str,
    pub supports_iso_dep: bool,
    pub supports_apdu_exchange: bool,
}

#[async_trait]
pub trait NfcReader: Send {
    fn capabilities(&self) -> ReaderCapabilities;

    async fn poll_card(&mut self) -> Result<Option<CardPresence>, ReaderError>;

    async fn power_off(&mut self) -> Result<(), ReaderError>;

    async fn exchange_apdu(&mut self, apdu: &[u8]) -> Result<Vec<u8>, ReaderError>;
}

#[async_trait]
pub trait ReaderFactory: Send + Sync {
    fn backend_name(&self) -> &'static str;

    async fn open(&self) -> Result<Box<dyn NfcReader>, ReaderError>;
}

#[derive(Debug, Error, Clone)]
pub enum ReaderError {
    #[error("reader configuration error: {0}")]
    Configuration(String),
    #[error("reader transport error: {0}")]
    Transport(String),
    #[error("reader protocol error: {0}")]
    Protocol(String),
    #[error("reader feature is not implemented yet: {0}")]
    Unsupported(String),
    #[error("reader I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for ReaderError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}
