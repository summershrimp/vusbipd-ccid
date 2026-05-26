use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use tracing::info;

use crate::ccid::CcidBridge;

use super::device::build_virtual_ccid_device;

pub struct UsbIpServer {
    listen_addr: SocketAddr,
    bridge: Arc<Mutex<CcidBridge>>,
}

impl UsbIpServer {
    pub fn new(listen_addr: SocketAddr, bridge: Arc<Mutex<CcidBridge>>) -> Self {
        Self {
            listen_addr,
            bridge,
        }
    }

    pub async fn run(self) -> Result<()> {
        let device = build_virtual_ccid_device(self.bridge);
        let server = Arc::new(::usbip::UsbIpServer::new_simulated(vec![device]));

        info!(listen_addr = %self.listen_addr, "USB/IP server ready with virtual CCID device");
        ::usbip::server(self.listen_addr, server).await;
        Ok(())
    }
}
