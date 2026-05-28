use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing::info;

use crate::{
    ccid::CcidBridge,
    config::{AppConfig, ReaderConfig},
    nfc::{ReaderFactory, dummy::DummyReaderFactory, pn532::Pn532UartFactory},
    stack::DependencyStack,
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
        let dependency_stack = DependencyStack::detect();
        info!(
            usbip = dependency_stack.usbip_crate,
            pn532 = dependency_stack.pn532_crate,
            apdu_command_capacity = dependency_stack.apdu_command_capacity,
            apdu_response_capacity = dependency_stack.apdu_response_capacity,
            ctaphid_message_capacity = dependency_stack.ctaphid_message_capacity,
            has_ctaphid_reference = dependency_stack.has_ctaphid_reference,
            "loaded third-party protocol stack"
        );

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

        let bridge = Arc::new(Mutex::new(CcidBridge::new(
            reader,
            self.config.poll_interval,
        )));
        let server = UsbIpServer::new(self.config.listen_addr, bridge);
        info!(listen_addr = %self.config.listen_addr, "starting USB/IP virtual CCID server");
        server.run().await
    }
}
