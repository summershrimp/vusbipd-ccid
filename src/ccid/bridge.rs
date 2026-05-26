use std::time::Duration;

use tracing::{debug, warn};

use crate::nfc::{CardPresence, NfcReader};

use super::protocol::{CcidCommand, CcidResponse, CommandStatus, IccStatus, SlotStatus};

const GENERIC_FAILURE_ERROR: u8 = 0xff;

pub struct CcidBridge {
    reader: Box<dyn NfcReader>,
    #[allow(dead_code)]
    poll_interval: Duration,
    current_card: Option<CardPresence>,
}

impl CcidBridge {
    pub fn new(reader: Box<dyn NfcReader>, poll_interval: Duration) -> Self {
        Self {
            reader,
            poll_interval,
            current_card: None,
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
                        self.current_card = Some(card);
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
                        self.current_card = None;
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

                self.current_card = None;
                CcidResponse::SlotStatus {
                    slot,
                    seq,
                    status: SlotStatus::ok(IccStatus::Inactive),
                    error: 0,
                    clock_status: 0,
                }
            }
            CcidCommand::GetSlotStatus { slot, seq } => {
                debug!(slot, seq, "handling CCID slot status request");
                match self.reader.poll_card() {
                    Ok(card) => {
                        let icc_status = if card.is_some() {
                            IccStatus::Active
                        } else {
                            IccStatus::NotPresent
                        };
                        self.current_card = card;
                        CcidResponse::SlotStatus {
                            slot,
                            seq,
                            status: SlotStatus::ok(icc_status),
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
                            status: SlotStatus::failed(Self::current_icc_status(
                                &self.current_card,
                            )),
                            error: GENERIC_FAILURE_ERROR,
                            clock_status: 0,
                        }
                    }
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
                        icc: Self::current_icc_status(&self.current_card),
                        command: CommandStatus::Failed,
                    },
                    error: GENERIC_FAILURE_ERROR,
                    clock_status: 0,
                }
            }
        }
    }

    fn current_icc_status(card: &Option<CardPresence>) -> IccStatus {
        if card.is_some() {
            IccStatus::Active
        } else {
            IccStatus::NotPresent
        }
    }

    fn build_pseudo_atr(card: &CardPresence) -> Vec<u8> {
        let historical_bytes = if card.historical_bytes.len() > 15 {
            &card.historical_bytes[..15]
        } else {
            &card.historical_bytes
        };

        let mut atr = vec![0x3b, 0x80 | historical_bytes.len() as u8];
        atr.extend_from_slice(historical_bytes);
        atr
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, time::Duration};

    use crate::{
        ccid::protocol::{CcidCommand, CcidResponse, IccStatus, SlotStatus},
        nfc::{CardPresence, CardProtocol, NfcReader, ReaderCapabilities, ReaderError},
    };

    use super::CcidBridge;

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
            historical_bytes: vec![0x80, 0x31],
        };

        let reader = FakeReader {
            poll_results: VecDeque::from([Ok(Some(card))]),
            exchange_result: Ok(vec![0x90, 0x00]),
        };
        let mut bridge = CcidBridge::new(Box::new(reader), Duration::from_millis(100));

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
                assert!(!payload.is_empty());
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }
}
