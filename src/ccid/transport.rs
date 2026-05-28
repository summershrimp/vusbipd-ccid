use std::{
    collections::VecDeque,
    io::{Error, ErrorKind, Result},
    sync::{Arc, Mutex},
};

use tracing::{debug, trace, warn};

use super::{
    CcidBridge,
    protocol::{CcidCommand, CcidResponse, CommandStatus, SlotStatus},
};

const CCID_MAX_MESSAGE_LENGTH: usize = 3072;
const CCID_GET_CLOCK_FREQUENCIES: u8 = 0x02;
const CCID_GET_DATA_RATES: u8 = 0x03;
const CCID_ABORT: u8 = 0x01;
const CCID_CLOCK_FREQUENCY_KHZ: [u8; 4] = [0xfc, 0x0d, 0x00, 0x00];
const CCID_DATA_RATE_BPS: [u8; 4] = [0x80, 0x25, 0x00, 0x00];
const CCID_NOTIFY_SLOT_CHANGE: u8 = 0x50;
const CMD_ABORTED_ERROR: u8 = 0xff;
const MAX_PENDING_IN_FRAMES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportState {
    Idle,
    Receiving,
    ReadyToSend,
}

pub struct CcidTransport {
    bridge: Arc<Mutex<CcidBridge>>,
    state: TransportState,
    pending_in_frames: VecDeque<Vec<u8>>,
    pending_interrupt_frames: VecDeque<Vec<u8>>,
    pending_out_buffer: Vec<u8>,
    next_frame_len: Option<usize>,
    slot_present: bool,
    bulk_abort: Option<u8>,
    control_abort: Option<(u8, u8)>,
}

impl std::fmt::Debug for CcidTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CcidTransport")
            .field("state", &self.state)
            .field("pending_in_frames", &self.pending_in_frames.len())
            .field(
                "pending_interrupt_frames",
                &self.pending_interrupt_frames.len(),
            )
            .field("pending_out_buffer", &self.pending_out_buffer.len())
            .field("next_frame_len", &self.next_frame_len)
            .field("slot_present", &self.slot_present)
            .finish()
    }
}

impl CcidTransport {
    pub fn new(bridge: Arc<Mutex<CcidBridge>>) -> Self {
        Self {
            bridge,
            state: TransportState::Idle,
            pending_in_frames: VecDeque::new(),
            pending_interrupt_frames: VecDeque::new(),
            pending_out_buffer: Vec::new(),
            next_frame_len: None,
            slot_present: false,
            bulk_abort: None,
            control_abort: None,
        }
    }

    pub fn handle_control_request(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
    ) -> Result<Vec<u8>> {
        debug!(request_type, request, "handling CCID control request");
        match (request_type, request) {
            (0xa1, CCID_GET_CLOCK_FREQUENCIES) => Ok(CCID_CLOCK_FREQUENCY_KHZ.to_vec()),
            (0xa1, CCID_GET_DATA_RATES) => Ok(CCID_DATA_RATE_BPS.to_vec()),
            (0x21, CCID_ABORT) => {
                let [slot, seq] = value.to_le_bytes();
                self.expect_abort(slot, seq);
                Ok(Vec::new())
            }
            _ => Err(Error::new(
                ErrorKind::Unsupported,
                format!(
                    "unsupported CCID control request type=0x{:02x} request=0x{:02x}",
                    request_type, request
                ),
            )),
        }
    }

    pub fn handle_bulk_out(&mut self, payload: &[u8]) -> Result<()> {
        trace!(payload_len = payload.len(), "handling CCID bulk OUT packet");
        self.pending_out_buffer.extend_from_slice(payload);
        self.update_state();

        while let Some(frame) = self.try_take_complete_frame()? {
            let command = CcidCommand::decode(&frame)
                .map_err(|error| Error::new(ErrorKind::InvalidData, error.to_string()))?;

            if let Some((slot, seq)) = self.control_abort {
                if matches!(command, CcidCommand::Abort { slot: bulk_slot, seq: bulk_seq } if bulk_slot == slot && bulk_seq == seq)
                {
                    self.finish_abort(slot, seq)?;
                } else {
                    self.queue_aborted_response(command)?;
                }
                continue;
            }

            self.bulk_abort = None;

            if let CcidCommand::Abort { slot, seq } = command {
                self.bulk_abort = Some(seq);
                let _ = slot;
                continue;
            }

            let (response, card_present) = {
                let mut bridge = self
                    .bridge
                    .lock()
                    .map_err(|_| Error::other("CCID bridge lock poisoned"))?;
                let response = bridge.handle_command(command);
                let card_present = bridge.card_present();
                (response, card_present)
            };

            self.enqueue_response(response, card_present);
        }

        Ok(())
    }

    pub fn handle_bulk_in(&mut self, max_len: usize) -> Vec<u8> {
        trace!(max_len, "handling CCID bulk IN packet");
        let Some(mut frame) = self.pending_in_frames.pop_front() else {
            trace!(state = ?self.state, "CCID bulk IN requested with no pending frames");
            self.update_state();
            return Vec::new();
        };

        if frame.len() <= max_len {
            if max_len != 0 && !frame.is_empty() && frame.len() % max_len == 0 {
                self.pending_in_frames.push_front(Vec::new());
            }
            trace!(?frame, remaining = self.pending_in_frames.len(), "serving CCID bulk IN frame");
            self.update_state();
            return frame;
        }

        let remainder = frame.split_off(max_len);
        self.pending_in_frames.push_front(remainder);
        trace!(?frame, remaining = self.pending_in_frames.len(), "serving fragmented CCID bulk IN frame");
        self.update_state();
        frame
    }

    pub fn handle_interrupt_in(&mut self) -> Vec<u8> {
        trace!("handling CCID interrupt IN packet");
        let card_presence = match self.bridge.lock() {
            Ok(mut bridge) => bridge.refresh_card_presence(),
            Err(_) => {
                warn!("failed to lock CCID bridge before interrupt IN");
                return self.pending_interrupt_frames.pop_front().unwrap_or_default();
            }
        };

        match card_presence {
            Ok(card_present) => self.update_slot_presence(card_present),
            Err(error) => warn!(?error, "failed to refresh card presence before interrupt IN"),
        }

        self.pending_interrupt_frames.pop_front().unwrap_or_default()
    }

    fn update_slot_presence(&mut self, present: bool) {
        if self.slot_present == present {
            self.pending_interrupt_frames.push_back(vec![
                CCID_NOTIFY_SLOT_CHANGE,
                if present { 0b0000_0001 } else { 0b0000_0000 },
            ]);
            return;
        }

        self.slot_present = present;
        self.pending_interrupt_frames.push_back(vec![
            CCID_NOTIFY_SLOT_CHANGE,
            if present { 0b0000_0011 } else { 0b0000_0010 },
        ]);
    }

    fn expect_abort(&mut self, slot: u8, seq: u8) {
        debug!(slot, seq, "expecting matching CCID bulk abort");
        if slot != 0 {
            return;
        }

        if self.bulk_abort == Some(seq) {
            if let Err(error) = self.finish_abort(slot, seq) {
                warn!(?error, slot, seq, "failed to complete CCID abort");
            }
        } else {
            self.control_abort = Some((slot, seq));
        }
    }

    fn finish_abort(&mut self, slot: u8, seq: u8) -> Result<()> {
        self.bulk_abort = None;
        self.control_abort = None;
        self.pending_out_buffer.clear();
        self.next_frame_len = None;
        self.pending_in_frames.clear();

        let (response, card_present) = {
            let mut bridge = self
                .bridge
                .lock()
                .map_err(|_| Error::other("CCID bridge lock poisoned"))?;
            let response = bridge.handle_command(CcidCommand::Abort { slot, seq });
            let card_present = bridge.card_present();
            (response, card_present)
        };

        self.enqueue_response(response, card_present);
        Ok(())
    }

    fn queue_aborted_response(&mut self, command: CcidCommand) -> Result<()> {
        let slot = command.slot();
        let seq = command.seq();
        let (encoded, card_present) = {
            let bridge = self
                .bridge
                .lock()
                .map_err(|_| Error::other("CCID bridge lock poisoned"))?;
            let status = SlotStatus {
                icc: bridge.current_icc_status(),
                command: CommandStatus::Failed,
            };
            let response = CcidResponse::SlotStatus {
                slot,
                seq,
                status,
                error: CMD_ABORTED_ERROR,
                clock_status: 0,
            };
            (response.encode(), bridge.card_present())
        };
        self.push_in_frame(encoded);
        self.update_slot_presence(card_present);
        self.update_state();
        Ok(())
    }

    fn enqueue_response(&mut self, response: CcidResponse, card_present: bool) {
        self.push_in_frame(response.encode());
        self.update_slot_presence(card_present);
        self.update_state();
    }

    fn push_in_frame(&mut self, frame: Vec<u8>) {
        if self.pending_in_frames.back().is_some_and(|pending| pending == &frame) {
            trace!(?frame, queued = self.pending_in_frames.len(), "dropping duplicate CCID bulk IN response");
            return;
        }

        if self.pending_in_frames.len() >= MAX_PENDING_IN_FRAMES {
            warn!(
                queued = self.pending_in_frames.len(),
                "dropping stale pending CCID bulk IN responses"
            );
            self.pending_in_frames.clear();
        }

        trace!(
            ?frame,
            queued = self.pending_in_frames.len() + 1,
            "queued CCID bulk IN response"
        );
        self.pending_in_frames.push_back(frame);
    }

    fn try_take_complete_frame(&mut self) -> Result<Option<Vec<u8>>> {
        if self.pending_out_buffer.len() < 10 {
            self.next_frame_len = None;
            self.update_state();
            return Ok(None);
        }

        let frame_len = if let Some(frame_len) = self.next_frame_len {
            frame_len
        } else {
            let payload_len = u32::from_le_bytes([
                self.pending_out_buffer[1],
                self.pending_out_buffer[2],
                self.pending_out_buffer[3],
                self.pending_out_buffer[4],
            ]) as usize;
            let frame_len = 10 + payload_len;

            if frame_len > CCID_MAX_MESSAGE_LENGTH + 10 {
                self.pending_out_buffer.clear();
                self.next_frame_len = None;
                self.update_state();
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("CCID frame length {frame_len} exceeds configured maximum"),
                ));
            }

            self.next_frame_len = Some(frame_len);
            frame_len
        };

        if self.pending_out_buffer.len() < frame_len {
            self.update_state();
            return Ok(None);
        }

        self.next_frame_len = None;
        let frame = self.pending_out_buffer.drain(..frame_len).collect();
        self.update_state();
        Ok(Some(frame))
    }

    fn update_state(&mut self) {
        self.state = if !self.pending_in_frames.is_empty() {
            TransportState::ReadyToSend
        } else if self.pending_out_buffer.is_empty() {
            self.next_frame_len = None;
            TransportState::Idle
        } else {
            TransportState::Receiving
        };
    }
}

impl CcidCommand {
    fn slot(&self) -> u8 {
        match self {
            Self::IccPowerOn { slot, .. }
            | Self::IccPowerOff { slot, .. }
            | Self::GetSlotStatus { slot, .. }
            | Self::GetParameters { slot, .. }
            | Self::ResetParameters { slot, .. }
            | Self::SetParameters { slot, .. }
            | Self::XfrBlock { slot, .. }
            | Self::Abort { slot, .. }
            | Self::Unknown { slot, .. } => *slot,
        }
    }

    fn seq(&self) -> u8 {
        match self {
            Self::IccPowerOn { seq, .. }
            | Self::IccPowerOff { seq, .. }
            | Self::GetSlotStatus { seq, .. }
            | Self::GetParameters { seq, .. }
            | Self::ResetParameters { seq, .. }
            | Self::SetParameters { seq, .. }
            | Self::XfrBlock { seq, .. }
            | Self::Abort { seq, .. }
            | Self::Unknown { seq, .. } => *seq,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        time::Duration,
    };

    use crate::{
        ccid::{
            CcidBridge,
            protocol::{
                CcidCommand, PC_TO_RDR_ABORT, PC_TO_RDR_GET_SLOT_STATUS, PC_TO_RDR_ICC_POWER_ON,
            },
        },
        nfc::{CardPresence, CardProtocol, NfcReader, ReaderCapabilities, ReaderError},
    };

    use super::{CcidTransport, TransportState};

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

    fn new_transport(
        poll_results: VecDeque<Result<Option<CardPresence>, ReaderError>>,
    ) -> CcidTransport {
        let bridge = Arc::new(Mutex::new(CcidBridge::new(
            Box::new(FakeReader { poll_results }),
            Duration::from_millis(100),
        )));
        CcidTransport::new(bridge)
    }

    use std::sync::{Arc, Mutex};

    #[test]
    fn bulk_out_queues_a_response_frame() {
        let mut transport = new_transport(VecDeque::from([Ok(Some(CardPresence {
            uid: vec![1, 2, 3, 4],
            protocol: CardProtocol::IsoDep,
            historical_bytes: vec![],
        }))]));

        let payload = vec![PC_TO_RDR_GET_SLOT_STATUS, 0, 0, 0, 0, 0, 1, 0, 0, 0];
        let _decoded = CcidCommand::decode(&payload).expect("payload must decode");
        transport
            .handle_bulk_out(&payload)
            .expect("bulk out must succeed");
        assert!(!transport.handle_bulk_in(64).is_empty());
    }

    #[test]
    fn fragmented_bulk_out_is_buffered_until_complete() {
        let mut transport = new_transport(VecDeque::from([Ok(Some(CardPresence {
            uid: vec![1, 2, 3, 4],
            protocol: CardProtocol::IsoDep,
            historical_bytes: vec![],
        }))]));

        let payload = vec![PC_TO_RDR_GET_SLOT_STATUS, 0, 0, 0, 0, 0, 1, 0, 0, 0];

        transport
            .handle_bulk_out(&payload[..4])
            .expect("first fragment must succeed");
        assert_eq!(transport.state, TransportState::Receiving);
        assert!(transport.handle_bulk_in(64).is_empty());

        transport
            .handle_bulk_out(&payload[4..])
            .expect("second fragment must succeed");
        assert_eq!(transport.state, TransportState::ReadyToSend);
        assert!(!transport.handle_bulk_in(64).is_empty());
    }

    #[test]
    fn slot_change_interrupt_is_generated_when_card_appears() {
        let mut transport = new_transport(VecDeque::from([Ok(Some(CardPresence {
            uid: vec![1, 2, 3, 4],
            protocol: CardProtocol::IsoDep,
            historical_bytes: vec![],
        }))]));

        let payload = vec![PC_TO_RDR_ICC_POWER_ON, 0, 0, 0, 0, 0, 1, 0, 0, 0];
        let _decoded = CcidCommand::decode(&payload).expect("payload must decode");

        transport
            .handle_bulk_out(&payload)
            .expect("bulk out must succeed");

        assert_eq!(transport.handle_interrupt_in(), vec![0x50, 0x03]);
    }

    #[test]
    fn interrupt_in_returns_empty_when_queue_is_empty() {
        let mut transport = new_transport(VecDeque::from([Ok(None)]));

        assert_eq!(transport.handle_interrupt_in(), vec![0x50, 0x00]);
        assert_eq!(transport.handle_interrupt_in(), vec![0x50, 0x00]);
    }

    #[test]
    fn control_abort_requires_matching_bulk_abort() {
        let mut transport = new_transport(VecDeque::from([Ok(None)]));

        transport
            .handle_control_request(0x21, 0x01, u16::from_le_bytes([0, 7]))
            .expect("control abort must succeed");
        transport
            .handle_bulk_out(&[PC_TO_RDR_GET_SLOT_STATUS, 0, 0, 0, 0, 0, 9, 0, 0, 0])
            .expect("bulk out must succeed");

        let response = transport.handle_bulk_in(64);
        assert_eq!(response[0], 0x81);
        assert_eq!(response[6], 9);
        assert_eq!(response[7] >> 6, 1);
        assert_eq!(response[8], 0xff);
    }

    #[test]
    fn matching_bulk_abort_completes_abort_sequence() {
        let mut transport = new_transport(VecDeque::from([Ok(None)]));

        transport
            .handle_control_request(0x21, 0x01, u16::from_le_bytes([0, 7]))
            .expect("control abort must succeed");
        transport
            .handle_bulk_out(&[PC_TO_RDR_ABORT, 0, 0, 0, 0, 0, 7, 0, 0, 0])
            .expect("matching bulk abort must succeed");

        let response = transport.handle_bulk_in(64);
        assert_eq!(response[0], 0x81);
        assert_eq!(response[6], 7);
        assert_eq!(response[7] >> 6, 0);
        assert_eq!(response[8], 0);
    }

    #[test]
    fn duplicate_slot_status_responses_are_coalesced() {
        let mut transport = new_transport(VecDeque::from([Ok(None), Ok(None)]));
        let payload = [PC_TO_RDR_GET_SLOT_STATUS, 0, 0, 0, 0, 0, 1, 0, 0, 0];

        transport
            .handle_bulk_out(&payload)
            .expect("first slot status request must succeed");
        transport
            .handle_bulk_out(&payload)
            .expect("second slot status request must succeed");

        assert_eq!(transport.pending_in_frames.len(), 1);
    }
}
