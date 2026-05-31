use tracing::{debug, warn};

use crate::nfc::{CardPresence, NfcReader};

use super::protocol::{CcidCommand, CcidResponse, CommandStatus, IccStatus, SlotStatus};

const GENERIC_FAILURE_ERROR: u8 = 0xff;
const T1_PROTOCOL_NUM: u8 = 0x01;
const DEFAULT_T1_PARAMETERS: [u8; 7] = [0x11, 0x10, 0x00, 0x15, 0x00, 0xfe, 0x00];
const APDU_SELECT_INS: u8 = 0xa4;
const APDU_SELECT_BY_DF_NAME_P1: u8 = 0x04;
const ATR_T0_HISTORICAL_BYTES_MASK: u8 = 0x0f;
const ATR_T0_TD1_PRESENT: u8 = 0x80;
const ATR_TD_T0_WITH_TD2: u8 = 0x80;
const ATR_TD_T1: u8 = 0x01;

pub struct CcidBridge {
    reader: Box<dyn NfcReader>,
    reader_capabilities: crate::nfc::ReaderCapabilities,
    current_card: Option<CardPresence>,
    slot_powered: bool,
    protocol_num: u8,
    parameters: [u8; 7],
}

impl CcidBridge {
    pub fn new(reader: Box<dyn NfcReader>) -> Self {
        let reader_capabilities = reader.capabilities();
        Self {
            reader,
            reader_capabilities,
            current_card: None,
            slot_powered: false,
            protocol_num: T1_PROTOCOL_NUM,
            parameters: DEFAULT_T1_PARAMETERS,
        }
    }

    pub fn handle_command(&mut self, command: CcidCommand) -> CcidResponse {
        match command {
            CcidCommand::IccPowerOn {
                slot,
                seq,
                power_select,
            } => {
                debug!(slot, seq, power_select, "handling CCID power on");
                match self.reader.poll_card() {
                    Ok(Some(card)) => {
                        let atr = Self::build_pseudo_atr(&card);
                        debug!(
                            uid = %format_hex(&card.uid),
                            atr = %format_hex(&atr),
                            historical_bytes = %format_hex(&card.historical_bytes),
                            "CCID power on found NFC card"
                        );
                        self.current_card = Some(card);
                        self.slot_powered = true;
                        CcidResponse::DataBlock {
                            slot,
                            seq,
                            status: SlotStatus::ok(IccStatus::Active),
                            error: 0,
                            chain_parameter: 0,
                            payload: atr,
                        }
                    }
                    Ok(None) => {
                        debug!("CCID power on found no NFC card");
                        self.current_card = None;
                        self.slot_powered = false;
                        CcidResponse::SlotStatus {
                            slot,
                            seq,
                            status: SlotStatus::failed(IccStatus::NotPresent),
                            error: GENERIC_FAILURE_ERROR,
                            clock_status: 0,
                        }
                    }
                    Err(error) => {
                        self.current_card = None;
                        self.slot_powered = false;
                        warn!(?error, "NFC power-on flow failed");
                        CcidResponse::SlotStatus {
                            slot,
                            seq,
                            status: SlotStatus::failed(IccStatus::Inactive),
                            error: GENERIC_FAILURE_ERROR,
                            clock_status: 0,
                        }
                    }
                }
            }
            CcidCommand::IccPowerOff { slot, seq } => {
                debug!(slot, seq, "handling CCID power off");
                if let Err(error) = self.reader.power_off() {
                    warn!(?error, "NFC power-off flow failed");
                    return CcidResponse::SlotStatus {
                        slot,
                        seq,
                        status: SlotStatus::failed(IccStatus::Inactive),
                        error: GENERIC_FAILURE_ERROR,
                        clock_status: 0,
                    };
                }

                self.slot_powered = false;
                CcidResponse::SlotStatus {
                    slot,
                    seq,
                    status: SlotStatus::ok(self.current_icc_status()),
                    error: 0,
                    clock_status: 0,
                }
            }
            CcidCommand::GetSlotStatus { slot, seq } => {
                debug!(slot, seq, "handling CCID slot status request");
                match self.refresh_card_presence() {
                    Ok(_) => {
                        if self.current_card.is_some() && self.reader_capabilities.name == "dummy" {
                            self.slot_powered = true;
                        }

                        CcidResponse::SlotStatus {
                            slot,
                            seq,
                            status: SlotStatus::ok(self.current_icc_status()),
                            error: 0,
                            clock_status: 0,
                        }
                    }
                    Err(error) => {
                        warn!(?error, "NFC slot status query failed");
                        CcidResponse::SlotStatus {
                            slot,
                            seq,
                            status: SlotStatus::failed(IccStatus::Inactive),
                            error: GENERIC_FAILURE_ERROR,
                            clock_status: 0,
                        }
                    }
                }
            }
            CcidCommand::GetParameters { slot, seq } => {
                debug!(slot, seq, "handling CCID get parameters request");
                self.parameters_response(slot, seq, false)
            }
            CcidCommand::ResetParameters { slot, seq } => {
                debug!(slot, seq, "handling CCID reset parameters request");
                self.protocol_num = T1_PROTOCOL_NUM;
                self.parameters = DEFAULT_T1_PARAMETERS;
                self.parameters_response(slot, seq, false)
            }
            CcidCommand::SetParameters {
                slot,
                seq,
                protocol_num,
                payload,
            } => {
                debug!(
                    slot,
                    seq,
                    protocol_num,
                    payload_len = payload.len(),
                    "handling CCID set parameters request"
                );
                if protocol_num != T1_PROTOCOL_NUM || payload.len() != DEFAULT_T1_PARAMETERS.len() {
                    return self.parameters_response(slot, seq, true);
                }

                self.protocol_num = protocol_num;
                self.parameters.copy_from_slice(&payload);
                self.parameters_response(slot, seq, false)
            }
            CcidCommand::XfrBlock {
                slot,
                seq,
                bwi,
                level_parameter,
                payload,
            } => {
                debug!(
                    slot,
                    seq,
                    bwi,
                    level_parameter,
                    payload_len = payload.len(),
                    "handling CCID APDU exchange"
                );

                if self.current_card.is_none() {
                    match self.refresh_card_presence() {
                        Ok(_) => {}
                        Err(error) => {
                            warn!(
                                ?error,
                                "NFC card presence check failed before APDU exchange"
                            );
                            self.slot_powered = false;
                            return CcidResponse::SlotStatus {
                                slot,
                                seq,
                                status: SlotStatus::failed(IccStatus::Inactive),
                                error: GENERIC_FAILURE_ERROR,
                                clock_status: 0,
                            };
                        }
                    }
                }

                if self.current_card.is_none() {
                    debug!("CCID APDU exchange blocked: no NFC card present");
                    return CcidResponse::SlotStatus {
                        slot,
                        seq,
                        status: SlotStatus::failed(IccStatus::NotPresent),
                        error: GENERIC_FAILURE_ERROR,
                        clock_status: 0,
                    };
                }

                if !self.slot_powered {
                    debug!("CCID APDU exchange blocked: slot is not powered");
                    return CcidResponse::SlotStatus {
                        slot,
                        seq,
                        status: SlotStatus::failed(IccStatus::Inactive),
                        error: GENERIC_FAILURE_ERROR,
                        clock_status: 0,
                    };
                }

                if let Some(aid) = Self::select_aid(&payload) {
                    debug!(aid = %format_hex(aid), "trying card AID");
                }

                match self.reader.exchange_apdu(&payload) {
                    Ok(response) => CcidResponse::DataBlock {
                        slot,
                        seq,
                        status: SlotStatus::ok(IccStatus::Active),
                        error: 0,
                        chain_parameter: 0,
                        payload: response,
                    },
                    Err(error) => {
                        warn!(?error, "NFC APDU exchange failed");
                        CcidResponse::SlotStatus {
                            slot,
                            seq,
                            status: SlotStatus::failed(self.current_icc_status()),
                            error: GENERIC_FAILURE_ERROR,
                            clock_status: 0,
                        }
                    }
                }
            }
            CcidCommand::Abort { slot, seq } => {
                debug!(slot, seq, "handling CCID abort request");
                CcidResponse::SlotStatus {
                    slot,
                    seq,
                    status: SlotStatus::ok(self.current_icc_status()),
                    error: 0,
                    clock_status: 0,
                }
            }
            CcidCommand::Unknown {
                message_type,
                slot,
                seq,
                ..
            } => {
                warn!(message_type, slot, seq, "unsupported CCID command");
                CcidResponse::SlotStatus {
                    slot,
                    seq,
                    status: SlotStatus {
                        icc: self.current_icc_status(),
                        command: CommandStatus::Failed,
                    },
                    error: GENERIC_FAILURE_ERROR,
                    clock_status: 0,
                }
            }
        }
    }

    pub fn card_present(&self) -> bool {
        self.current_card.is_some()
    }

    pub fn refresh_card_presence(&mut self) -> Result<bool, crate::nfc::ReaderError> {
        if self.slot_powered && self.current_card.is_some() {
            return Ok(true);
        }

        let card = self.reader.poll_card()?;
        if card.is_none() {
            self.slot_powered = false;
        }
        self.current_card = card;
        Ok(self.current_card.is_some())
    }

    pub(crate) fn current_icc_status(&self) -> IccStatus {
        if self.current_card.is_none() {
            IccStatus::NotPresent
        } else if self.slot_powered {
            IccStatus::Active
        } else {
            IccStatus::Inactive
        }
    }

    fn parameters_response(&self, slot: u8, seq: u8, failed: bool) -> CcidResponse {
        CcidResponse::Parameters {
            slot,
            seq,
            status: if failed {
                SlotStatus::failed(self.current_icc_status())
            } else {
                SlotStatus::ok(self.current_icc_status())
            },
            error: if failed { GENERIC_FAILURE_ERROR } else { 0 },
            protocol_num: self.protocol_num,
            payload: self.parameters.to_vec(),
        }
    }

    fn build_pseudo_atr(card: &CardPresence) -> Vec<u8> {
        if card
            .historical_bytes
            .first()
            .is_some_and(|byte| matches!(byte, 0x3b | 0x3f))
        {
            return card.historical_bytes.clone();
        }

        let historical_bytes =
            if card.historical_bytes.len() > ATR_T0_HISTORICAL_BYTES_MASK as usize {
                &card.historical_bytes[..15]
            } else {
                &card.historical_bytes
            };

        let mut atr = vec![
            0x3b,
            ATR_T0_TD1_PRESENT | historical_bytes.len() as u8,
            ATR_TD_T0_WITH_TD2,
            ATR_TD_T1,
        ];
        atr.extend_from_slice(historical_bytes);

        let checksum = atr
            .iter()
            .skip(1)
            .fold(0u8, |checksum, byte| checksum ^ byte);
        atr.push(checksum);
        atr
    }

    fn select_aid(apdu: &[u8]) -> Option<&[u8]> {
        if apdu.len() < 5 || apdu[1] != APDU_SELECT_INS || apdu[2] != APDU_SELECT_BY_DF_NAME_P1 {
            return None;
        }

        let aid_len = apdu[4] as usize;
        let aid_start = 5;
        let aid_end = aid_start + aid_len;

        (aid_len > 0 && apdu.len() >= aid_end).then_some(&apdu[aid_start..aid_end])
    }
}

fn format_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::{
        ccid::protocol::{CcidCommand, CcidResponse, IccStatus, SlotStatus},
        nfc::{CardPresence, CardProtocol, NfcReader, ReaderCapabilities, ReaderError},
    };

    use super::{format_hex, CcidBridge};

    struct FakeReader {
        poll_results: VecDeque<Result<Option<CardPresence>, ReaderError>>,
        exchange_result: Result<Vec<u8>, ReaderError>,
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
            self.exchange_result.clone()
        }
    }

    #[test]
    fn power_on_returns_data_block_when_card_exists() {
        let card = CardPresence {
            uid: vec![0x01, 0x02, 0x03, 0x04],
            protocol: CardProtocol::IsoDep,
            historical_bytes: vec![0x3b, 0x68, 0x00, 0xff, 0x38, 0x2b, 0x41, 0x52, 0x44, 0x6e, 0x73, 0x73],
        };

        let reader = FakeReader {
            poll_results: VecDeque::from([Ok(Some(card))]),
            exchange_result: Ok(vec![0x90, 0x00]),
        };
        let mut bridge = CcidBridge::new(Box::new(reader));

        let response = bridge.handle_command(CcidCommand::IccPowerOn {
            slot: 0,
            seq: 7,
            power_select: 0,
        });

        match response {
            CcidResponse::DataBlock {
                status, payload, ..
            } => {
                assert_eq!(status, SlotStatus::ok(IccStatus::Active));
                assert_eq!(payload, vec![0x3b, 0x68, 0x00, 0xff, 0x38, 0x2b, 0x41, 0x52, 0x44, 0x6e, 0x73, 0x73]);
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn power_off_keeps_card_present_but_inactive() {
        let card = CardPresence {
            uid: vec![0x01, 0x02, 0x03, 0x04],
            protocol: CardProtocol::IsoDep,
            historical_bytes: vec![0x3b, 0x68, 0x00, 0xff, 0x38, 0x2b, 0x41, 0x52, 0x44, 0x6e, 0x73, 0x73],
        };

        let reader = FakeReader {
            poll_results: VecDeque::from([Ok(Some(card.clone())), Ok(Some(card))]),
            exchange_result: Ok(vec![0x90, 0x00]),
        };
        let mut bridge = CcidBridge::new(Box::new(reader));

        let _ = bridge.handle_command(CcidCommand::IccPowerOn {
            slot: 0,
            seq: 1,
            power_select: 0,
        });
        let response = bridge.handle_command(CcidCommand::IccPowerOff { slot: 0, seq: 2 });

        match response {
            CcidResponse::SlotStatus { status, .. } => {
                assert_eq!(status, SlotStatus::ok(IccStatus::Inactive));
                assert!(bridge.card_present());
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn refresh_does_not_poll_while_powered_card_is_connected() {
        let card = CardPresence {
            uid: vec![0x01, 0x02, 0x03, 0x04],
            protocol: CardProtocol::IsoDep,
            historical_bytes: vec![0x3b, 0x68, 0x00, 0xff],
        };

        let reader = FakeReader {
            poll_results: VecDeque::from([Ok(Some(card)), Ok(None)]),
            exchange_result: Ok(vec![0x90, 0x00]),
        };
        let mut bridge = CcidBridge::new(Box::new(reader));

        let _ = bridge.handle_command(CcidCommand::IccPowerOn {
            slot: 0,
            seq: 1,
            power_select: 0,
        });

        assert!(bridge.refresh_card_presence().expect("refresh must succeed"));
        assert!(bridge.card_present());
        assert_eq!(bridge.current_icc_status(), IccStatus::Active);
    }

    #[test]
    fn get_parameters_returns_t1_defaults() {
        let reader = FakeReader {
            poll_results: VecDeque::new(),
            exchange_result: Ok(vec![0x90, 0x00]),
        };
        let mut bridge = CcidBridge::new(Box::new(reader));

        let response = bridge.handle_command(CcidCommand::GetParameters { slot: 0, seq: 1 });

        match response {
            CcidResponse::Parameters {
                protocol_num,
                payload,
                ..
            } => {
                assert_eq!(protocol_num, 1);
                assert_eq!(payload, vec![0x11, 0x10, 0x00, 0x15, 0x00, 0xfe, 0x00]);
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn build_pseudo_atr_declares_t1_protocol() {
        let card = CardPresence {
            uid: vec![0x27, 0x9c, 0x8b, 0x22, 0x02, 0x8b, 0x9c],
            protocol: CardProtocol::IsoDep,
            historical_bytes: vec![
                0x80, 0x73, 0xc0, 0x21, 0xc0, 0x57, 0x59, 0x75, 0x62, 0x69, 0x4b, 0x65, 0x79,
            ],
        };

        let atr = CcidBridge::build_pseudo_atr(&card);

        assert_eq!(atr[0], 0x3b);
        assert_eq!(atr[1], 0x8d);
        assert_eq!(atr[2], 0x80);
        assert_eq!(atr[3], 0x01);
        assert_eq!(&atr[4..17], card.historical_bytes.as_slice());
    }

    #[test]
    fn select_aid_extracts_df_name() {
        let apdu = [0x00, 0xa4, 0x04, 0x00, 0x03, 0xa0, 0x00, 0x01, 0x00];

        assert_eq!(CcidBridge::select_aid(&apdu), Some([0xa0, 0x00, 0x01].as_slice()));
    }

    #[test]
    fn format_hex_returns_lowercase_hex() {
        assert_eq!(format_hex(&[0xa0, 0x00, 0x01]), "a00001");
    }
}
