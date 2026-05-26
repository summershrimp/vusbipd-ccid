use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use tracing::{debug, info, warn};

use crate::ccid::CcidBridge;

pub struct UsbIpServer {
    listen_addr: SocketAddr,
    #[allow(dead_code)]
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
        let listener = TcpListener::bind(self.listen_addr).await?;
        info!(listen_addr = %self.listen_addr, "USB/IP listener bound");

        loop {
            let (stream, peer_addr) = listener.accept().await?;
            debug!(%peer_addr, "accepted USB/IP client connection");
            tokio::spawn(async move {
                if let Err(error) = handle_client(stream).await {
                    warn!(%peer_addr, ?error, "USB/IP client handler ended with error");
                }
            });
        }
    }
}

async fn handle_client(mut stream: TcpStream) -> Result<()> {
    let mut probe = [0u8; 4];
    let bytes_read = stream.read(&mut probe).await?;

    if bytes_read == 0 {
        return Ok(());
    }

    warn!(
        bytes_read,
        first_bytes = ?&probe[..bytes_read],
        "USB/IP protocol handling is not implemented yet; closing connection"
    );

    Ok(())
}
