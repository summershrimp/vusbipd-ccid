use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing::info;

use crate::{
    ccid::CcidBridge,
    config::{AppConfig, ReaderConfig},
    nfc::{ReaderFactory, dummy::DummyReaderFactory, pn532::Pn532UartFactory},
    usbip::UsbIpServer,
};

pub struct Application {
    config: AppConfig,
}

impl Application {
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }

    pub async fn run(self) -> Result<()> {
        let reader = match self.config.reader.clone() {
            ReaderConfig::Pn532Uart(config) => {
                let factory = Pn532UartFactory::new(config);
                info!(
                    backend = factory.backend_name(),
                    "opening NFC reader backend"
                );
                factory.open()?
            }
            ReaderConfig::Dummy => {
                let factory = DummyReaderFactory;
                info!(
                    backend = factory.backend_name(),
                    "opening dummy NFC reader backend"
                );
                factory.open()?
            }
        };

        let bridge = Arc::new(Mutex::new(CcidBridge::new(reader)));
        let server = UsbIpServer::new(self.config.listen_addr, bridge);
        info!(listen_addr = %self.config.listen_addr, "starting USB/IP virtual CCID server");
        server.run().await
    }
}
