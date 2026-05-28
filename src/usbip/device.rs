use std::{
    any::Any,
    io::{Error, ErrorKind, Result},
    sync::{Arc, Mutex},
};

use usbip::UsbSpeed::Full;
use ::usbip::{
    Direction, EndpointAttributes, SetupPacket, StandardRequest, UsbDevice, UsbDeviceHandler,
    UsbEndpoint, UsbInterface, UsbInterfaceHandler,
};
use tracing::{warn, trace};

use crate::ccid::{CcidBridge, CcidTransport};

const CCID_INTERFACE_CLASS: u8 = 0x0b;
const CCID_INTERFACE_SUBCLASS: u8 = 0x00;
const CCID_INTERFACE_PROTOCOL: u8 = 0x00;
const CCID_BULK_IN_ENDPOINT: u8 = 0x81;
const CCID_BULK_OUT_ENDPOINT: u8 = 0x01;
const CCID_INTERRUPT_IN_ENDPOINT: u8 = 0x82;
const CCID_FUNCTIONAL_DESCRIPTOR_TYPE: u8 = 0x21;
const CCID_CLOCK_FREQUENCY_KHZ: [u8; 4] = [0xfc, 0x0d, 0x00, 0x00];
const CCID_DATA_RATE_BPS: [u8; 4] = [0x80, 0x25, 0x00, 0x00];
const USB_FEATURE_REMOTE_WAKEUP: u16 = 0x0001;

pub fn build_virtual_ccid_device(bridge: Arc<Mutex<CcidBridge>>) -> UsbDevice {
    let handler = Arc::new(Mutex::new(
        Box::new(CcidUsbIpInterfaceHandler::new(bridge)) as Box<dyn UsbInterfaceHandler + Send>
    ));
    let device_handler = Arc::new(Mutex::new(
        Box::new(CcidUsbIpDeviceHandler::default()) as Box<dyn UsbDeviceHandler + Send>
    ));

    let mut device = UsbDevice::new(0)
        .with_device_handler(device_handler)
        .with_interface(
            CCID_INTERFACE_CLASS,
            CCID_INTERFACE_SUBCLASS,
            CCID_INTERFACE_PROTOCOL,
            Some("Virtual CCID Reader"),
            CcidUsbIpInterfaceHandler::endpoints(),
            handler,
        );
    device.usb_version.major = 2;
    device.usb_version.minor = 0;
    device.speed = Full as u32;
    device.bus_id = "1-1".to_string();
    device.bus_num = 1;
    device.dev_num = 2;
    device.path = "/sys/bus/usb/0/0".to_string();
    device.vendor_id = 0xffff;
    device.product_id = 0x0001;
    device.device_class = 0x00;
    device.device_subclass = 0x00;
    device.device_protocol = 0x00;
    device.set_manufacturer_name("vusbipd-ccid");
    device.set_product_name("Virtual CCID over USB/IP");
    device.set_serial_number("0123456789abcdef");
    device
}

#[derive(Debug, Default)]
struct CcidUsbIpDeviceHandler {
    remote_wakeup_enabled: bool,
}

impl CcidUsbIpDeviceHandler {
    fn limited_response(
        transfer_buffer_length: u32,
        setup: SetupPacket,
        mut payload: Vec<u8>,
    ) -> Vec<u8> {
        let max_len = usize::min(transfer_buffer_length as usize, setup.length as usize);
        payload.truncate(max_len);
        payload
    }
}

impl UsbDeviceHandler for CcidUsbIpDeviceHandler {
    fn handle_urb(
        &mut self,
        transfer_buffer_length: u32,
        setup: SetupPacket,
        _req: &[u8],
    ) -> Result<Vec<u8>> {
        trace!(setup.request_type, setup.request, "handling device control request");
        match (setup.request_type, setup.request) {
            (0x80, request) if request == StandardRequest::GetStatus as u8 => {
                let status = if self.remote_wakeup_enabled { 0x0002u16 } else { 0x0000u16 };
                Ok(Self::limited_response(
                    transfer_buffer_length,
                    setup,
                    status.to_le_bytes().to_vec(),
                ))
            }
            (0x80, request) if request == StandardRequest::GetConfiguration as u8 => Ok(
                Self::limited_response(
                transfer_buffer_length,
                setup,
                vec![1],
            )),
            (0x00, request)
                if request == StandardRequest::SetFeature as u8
                    && setup.value == USB_FEATURE_REMOTE_WAKEUP =>
            {
                self.remote_wakeup_enabled = true;
                Ok(Vec::new())
            }
            (0x00, request)
                if request == StandardRequest::ClearFeature as u8
                    && setup.value == USB_FEATURE_REMOTE_WAKEUP =>
            {
                self.remote_wakeup_enabled = false;
                Ok(Vec::new())
            }
            _ => Err(Error::new(
                ErrorKind::Unsupported,
                format!("unsupported device request: {setup:?}"),
            )),
        }
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

struct CcidUsbIpInterfaceHandler {
    transport: CcidTransport,
}

impl std::fmt::Debug for CcidUsbIpInterfaceHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CcidUsbIpInterfaceHandler")
            .field("transport", &self.transport)
            .finish()
    }
}

impl CcidUsbIpInterfaceHandler {
    fn new(bridge: Arc<Mutex<CcidBridge>>) -> Self {
        Self {
            transport: CcidTransport::new(bridge),
        }
    }

    fn endpoints() -> Vec<UsbEndpoint> {
        vec![
            UsbEndpoint {
                address: CCID_BULK_OUT_ENDPOINT,
                attributes: EndpointAttributes::Bulk as u8,
                max_packet_size: 64,
                interval: 0,
            },
            UsbEndpoint {
                address: CCID_BULK_IN_ENDPOINT,
                attributes: EndpointAttributes::Bulk as u8,
                max_packet_size: 64,
                interval: 0,
            },
            UsbEndpoint {
                address: CCID_INTERRUPT_IN_ENDPOINT,
                attributes: EndpointAttributes::Interrupt as u8,
                max_packet_size: 8,
                interval: 255,
            },
        ]
    }

    fn class_specific_descriptor() -> Vec<u8> {
        vec![
            /* bLenght */ 0x36,
            /* bDescriptorType */CCID_FUNCTIONAL_DESCRIPTOR_TYPE,
            0x10, 0x01,
            0x00,
            0x01,
            0x02, 0x00, 0x00, 0x00,
            CCID_CLOCK_FREQUENCY_KHZ[0], CCID_CLOCK_FREQUENCY_KHZ[1],
            CCID_CLOCK_FREQUENCY_KHZ[2], CCID_CLOCK_FREQUENCY_KHZ[3],
            CCID_CLOCK_FREQUENCY_KHZ[0], CCID_CLOCK_FREQUENCY_KHZ[1],
            CCID_CLOCK_FREQUENCY_KHZ[2], CCID_CLOCK_FREQUENCY_KHZ[3],
            0x00,
            CCID_DATA_RATE_BPS[0], CCID_DATA_RATE_BPS[1],
            CCID_DATA_RATE_BPS[2], CCID_DATA_RATE_BPS[3],
            CCID_DATA_RATE_BPS[0], CCID_DATA_RATE_BPS[1],
            CCID_DATA_RATE_BPS[2], CCID_DATA_RATE_BPS[3],
            0x00,
            0xfe, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            0x40, 0x08, 0x04, 0x00,
            0x00, 0x0c, 0x00, 0x00,
            0xff,
            0xff,
            0x00, 0x00,
            0x00,
            0x01,
        ]
    }

    fn handle_bulk_out(&mut self, payload: &[u8]) -> Result<Vec<u8>> {
        self.transport.handle_bulk_out(payload)?;
        Ok(Vec::new())
    }

    fn handle_bulk_in(&mut self, max_len: usize) -> Vec<u8> {
        self.transport.handle_bulk_in(max_len)
    }

    fn handle_interrupt_in(&mut self) -> Vec<u8> {
        self.transport.handle_interrupt_in()
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
            return self
                .transport
                .handle_control_request(setup.request_type, setup.request, setup.value);
        }

        match (ep.direction(), ep.address) {
            (Direction::Out, CCID_BULK_OUT_ENDPOINT) => self.handle_bulk_out(req),
            (Direction::In, CCID_BULK_IN_ENDPOINT) => {
                Ok(self.handle_bulk_in(transfer_buffer_length as usize))
            }
            (Direction::In, CCID_INTERRUPT_IN_ENDPOINT) => Ok(self.handle_interrupt_in()),
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
    use usbip::{SetupPacket, StandardRequest, UsbDeviceHandler};

    use super::{CcidUsbIpDeviceHandler, CcidUsbIpInterfaceHandler, USB_FEATURE_REMOTE_WAKEUP};

    #[test]
    fn class_specific_descriptor_has_expected_length() {
        assert_eq!(
            CcidUsbIpInterfaceHandler::class_specific_descriptor().len(),
            54
        );
    }

    #[test]
    fn device_handler_reports_default_device_status() {
        let mut handler = CcidUsbIpDeviceHandler::default();

        let response = handler
            .handle_urb(
                64,
                SetupPacket {
                    request_type: 0x80,
                    request: StandardRequest::GetStatus as u8,
                    value: 0,
                    index: 0,
                    length: 2,
                },
                &[],
            )
            .expect("GET_STATUS must succeed");

        assert_eq!(response, vec![0x00, 0x00]);
    }

    #[test]
    fn device_handler_tracks_remote_wakeup_feature() {
        let mut handler = CcidUsbIpDeviceHandler::default();

        handler
            .handle_urb(
                0,
                SetupPacket {
                    request_type: 0x00,
                    request: StandardRequest::SetFeature as u8,
                    value: USB_FEATURE_REMOTE_WAKEUP,
                    index: 0,
                    length: 0,
                },
                &[],
            )
            .expect("SET_FEATURE must succeed");

        let response = handler
            .handle_urb(
                64,
                SetupPacket {
                    request_type: 0x80,
                    request: StandardRequest::GetStatus as u8,
                    value: 0,
                    index: 0,
                    length: 2,
                },
                &[],
            )
            .expect("GET_STATUS must succeed");

        assert_eq!(response, vec![0x02, 0x00]);
    }
}
