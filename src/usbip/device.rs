use std::{
    any::Any,
    collections::VecDeque,
    io::{Error, ErrorKind, Result},
    sync::{Arc, Mutex},
};

use ::usbip::{
    Direction, EndpointAttributes, SetupPacket, UsbDevice, UsbEndpoint, UsbInterface,
    UsbInterfaceHandler,
};
use tracing::{debug, warn};

use crate::ccid::{CcidBridge, protocol::CcidCommand};

const CCID_INTERFACE_CLASS: u8 = 0x0b;
const CCID_INTERFACE_SUBCLASS: u8 = 0x00;
const CCID_INTERFACE_PROTOCOL: u8 = 0x00;
const CCID_GET_CLOCK_FREQUENCIES: u8 = 0x02;
const CCID_GET_DATA_RATES: u8 = 0x03;
const CCID_ABORT: u8 = 0x01;
const CCID_FUNCTIONAL_DESCRIPTOR_TYPE: u8 = 0x21;
const CCID_CLOCK_FREQUENCY_KHZ: [u8; 4] = [0xfc, 0x0d, 0x00, 0x00];
const CCID_DATA_RATE_BPS: [u8; 4] = [0x80, 0x25, 0x00, 0x00];

pub fn build_virtual_ccid_device(bridge: Arc<Mutex<CcidBridge>>) -> UsbDevice {
    let handler = Arc::new(Mutex::new(
        Box::new(CcidUsbIpInterfaceHandler::new(bridge)) as Box<dyn UsbInterfaceHandler + Send>
    ));

    let mut device = UsbDevice::new(0).with_interface(
        CCID_INTERFACE_CLASS,
        CCID_INTERFACE_SUBCLASS,
        CCID_INTERFACE_PROTOCOL,
        Some("Virtual CCID Reader"),
        CcidUsbIpInterfaceHandler::endpoints(),
        handler,
    );

    device.vendor_id = 0xffff;
    device.product_id = 0x0001;
    device.device_class = 0x00;
    device.device_subclass = 0x00;
    device.device_protocol = 0x00;
    device.set_manufacturer_name("vusbipd-ccid");
    device.set_product_name("Virtual CCID over USB/IP");
    device.set_serial_number("debug-virtual-ccid");
    device
}

struct CcidUsbIpInterfaceHandler {
    bridge: Arc<Mutex<CcidBridge>>,
    pending_in_frames: VecDeque<Vec<u8>>,
}

impl std::fmt::Debug for CcidUsbIpInterfaceHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CcidUsbIpInterfaceHandler")
            .field("pending_in_frames", &self.pending_in_frames.len())
            .finish()
    }
}

impl CcidUsbIpInterfaceHandler {
    fn new(bridge: Arc<Mutex<CcidBridge>>) -> Self {
        Self {
            bridge,
            pending_in_frames: VecDeque::new(),
        }
    }

    fn endpoints() -> Vec<UsbEndpoint> {
        vec![
            UsbEndpoint {
                address: 0x81,
                attributes: EndpointAttributes::Bulk as u8,
                max_packet_size: 64,
                interval: 0,
            },
            UsbEndpoint {
                address: 0x01,
                attributes: EndpointAttributes::Bulk as u8,
                max_packet_size: 64,
                interval: 0,
            },
            UsbEndpoint {
                address: 0x82,
                attributes: EndpointAttributes::Interrupt as u8,
                max_packet_size: 8,
                interval: 32,
            },
        ]
    }

    fn class_specific_descriptor() -> Vec<u8> {
        vec![
            0x36,
            CCID_FUNCTIONAL_DESCRIPTOR_TYPE,
            0x10,
            0x01,
            0x00,
            0x01,
            0x02,
            0x00,
            0x00,
            0x00,
            CCID_CLOCK_FREQUENCY_KHZ[0],
            CCID_CLOCK_FREQUENCY_KHZ[1],
            CCID_CLOCK_FREQUENCY_KHZ[2],
            CCID_CLOCK_FREQUENCY_KHZ[3],
            CCID_CLOCK_FREQUENCY_KHZ[0],
            CCID_CLOCK_FREQUENCY_KHZ[1],
            CCID_CLOCK_FREQUENCY_KHZ[2],
            CCID_CLOCK_FREQUENCY_KHZ[3],
            0x00,
            CCID_DATA_RATE_BPS[0],
            CCID_DATA_RATE_BPS[1],
            CCID_DATA_RATE_BPS[2],
            CCID_DATA_RATE_BPS[3],
            CCID_DATA_RATE_BPS[0],
            CCID_DATA_RATE_BPS[1],
            CCID_DATA_RATE_BPS[2],
            CCID_DATA_RATE_BPS[3],
            0x00,
            0xfe,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x40,
            0x08,
            0x04,
            0x00,
            0x00,
            0x0c,
            0x00,
            0x00,
            0xff,
            0xff,
            0x00,
            0x00,
            0x00,
            0x01,
        ]
    }

    fn handle_control_request(&mut self, setup: SetupPacket) -> Result<Vec<u8>> {
        debug!(?setup, "handling CCID control request");
        match (setup.request_type, setup.request) {
            (0xa1, CCID_GET_CLOCK_FREQUENCIES) => Ok(CCID_CLOCK_FREQUENCY_KHZ.to_vec()),
            (0xa1, CCID_GET_DATA_RATES) => Ok(CCID_DATA_RATE_BPS.to_vec()),
            (0x21, CCID_ABORT) => Ok(Vec::new()),
            _ => Err(Error::new(
                ErrorKind::Unsupported,
                format!(
                    "unsupported CCID control request type=0x{:02x} request=0x{:02x}",
                    setup.request_type, setup.request
                ),
            )),
        }
    }

    fn handle_bulk_out(&mut self, payload: &[u8]) -> Result<Vec<u8>> {
        debug!(payload_len = payload.len(), "handling CCID bulk OUT packet");
        let command = CcidCommand::decode(payload)
            .map_err(|error| Error::new(ErrorKind::InvalidData, error.to_string()))?;

        let response = self
            .bridge
            .lock()
            .map_err(|_| Error::other("CCID bridge lock poisoned"))?
            .handle_command(command)
            .encode();

        self.pending_in_frames.push_back(response);
        Ok(Vec::new())
    }

    fn handle_bulk_in(&mut self, max_len: usize) -> Vec<u8> {
        let Some(mut frame) = self.pending_in_frames.pop_front() else {
            return Vec::new();
        };

        if frame.len() <= max_len {
            return frame;
        }

        let remainder = frame.split_off(max_len);
        self.pending_in_frames.push_front(remainder);
        frame
    }
}

impl UsbInterfaceHandler for CcidUsbIpInterfaceHandler {
    fn get_class_specific_descriptor(&self) -> Vec<u8> {
        Self::class_specific_descriptor()
    }

    fn handle_urb(
        &mut self,
        _interface: &UsbInterface,
        ep: UsbEndpoint,
        transfer_buffer_length: u32,
        setup: SetupPacket,
        req: &[u8],
    ) -> Result<Vec<u8>> {
        if ep.is_ep0() {
            return self.handle_control_request(setup);
        }

        match (ep.direction(), ep.address) {
            (Direction::Out, 0x01) => self.handle_bulk_out(req),
            (Direction::In, 0x81) => Ok(self.handle_bulk_in(transfer_buffer_length as usize)),
            (Direction::In, 0x82) => Ok(Vec::new()),
            _ => {
                warn!(
                    address = ep.address,
                    "received URB for unsupported CCID endpoint"
                );
                Ok(Vec::new())
            }
        }
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use crate::{
        ccid::{
            CcidBridge,
            protocol::{CcidCommand, PC_TO_RDR_GET_SLOT_STATUS},
        },
        nfc::{CardPresence, CardProtocol, NfcReader, ReaderCapabilities, ReaderError},
    };

    use super::CcidUsbIpInterfaceHandler;

    struct FakeReader {
        poll_results: VecDeque<Result<Option<CardPresence>, ReaderError>>,
    }

    impl NfcReader for FakeReader {
        fn capabilities(&self) -> ReaderCapabilities {
            ReaderCapabilities {
                name: "fake",
                supports_iso_dep: true,
                supports_apdu_exchange: true,
            }
        }

        fn poll_card(&mut self) -> Result<Option<CardPresence>, ReaderError> {
            self.poll_results.pop_front().unwrap_or_else(|| Ok(None))
        }

        fn power_off(&mut self) -> Result<(), ReaderError> {
            Ok(())
        }

        fn exchange_apdu(&mut self, _apdu: &[u8]) -> Result<Vec<u8>, ReaderError> {
            Ok(vec![0x90, 0x00])
        }
    }

    #[test]
    fn class_specific_descriptor_has_expected_length() {
        assert_eq!(
            CcidUsbIpInterfaceHandler::class_specific_descriptor().len(),
            54
        );
    }

    #[test]
    fn bulk_out_queues_a_response_frame() {
        let bridge = Arc::new(Mutex::new(CcidBridge::new(
            Box::new(FakeReader {
                poll_results: VecDeque::from([Ok(Some(CardPresence {
                    uid: vec![1, 2, 3, 4],
                    protocol: CardProtocol::IsoDep,
                    historical_bytes: vec![],
                }))]),
            }),
            Duration::from_millis(100),
        )));

        let mut handler = CcidUsbIpInterfaceHandler::new(bridge);
        let payload = vec![PC_TO_RDR_GET_SLOT_STATUS, 0, 0, 0, 0, 0, 1, 0, 0, 0];
        let _decoded = CcidCommand::decode(&payload).expect("payload must decode");
        handler
            .handle_bulk_out(&payload)
            .expect("bulk out must succeed");
        assert!(!handler.handle_bulk_in(64).is_empty());
    }
}
