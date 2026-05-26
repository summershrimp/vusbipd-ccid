use thiserror::Error;

pub const PC_TO_RDR_ICC_POWER_ON: u8 = 0x62;
pub const PC_TO_RDR_ICC_POWER_OFF: u8 = 0x63;
pub const PC_TO_RDR_GET_SLOT_STATUS: u8 = 0x65;
pub const PC_TO_RDR_XFR_BLOCK: u8 = 0x6f;

pub const RDR_TO_PC_DATA_BLOCK: u8 = 0x80;
pub const RDR_TO_PC_SLOT_STATUS: u8 = 0x81;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CcidCommand {
    IccPowerOn {
        slot: u8,
        seq: u8,
        power_select: u8,
    },
    IccPowerOff {
        slot: u8,
        seq: u8,
    },
    GetSlotStatus {
        slot: u8,
        seq: u8,
    },
    XfrBlock {
        slot: u8,
        seq: u8,
        bwi: u8,
        level_parameter: u16,
        payload: Vec<u8>,
    },
    Unknown {
        message_type: u8,
        slot: u8,
        seq: u8,
        parameters: [u8; 3],
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CcidResponse {
    DataBlock {
        slot: u8,
        seq: u8,
        status: SlotStatus,
        error: u8,
        chain_parameter: u8,
        payload: Vec<u8>,
    },
    SlotStatus {
        slot: u8,
        seq: u8,
        status: SlotStatus,
        error: u8,
        clock_status: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IccStatus {
    Active = 0,
    Inactive = 1,
    NotPresent = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus {
    NoError = 0,
    Failed = 1,
    TimeExtension = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotStatus {
    pub icc: IccStatus,
    pub command: CommandStatus,
}

impl SlotStatus {
    pub fn ok(icc: IccStatus) -> Self {
        Self {
            icc,
            command: CommandStatus::NoError,
        }
    }

    pub fn failed(icc: IccStatus) -> Self {
        Self {
            icc,
            command: CommandStatus::Failed,
        }
    }

    pub fn encode(self) -> u8 {
        ((self.command as u8) << 6) | (self.icc as u8)
    }
}

#[derive(Debug, Error)]
pub enum CcidProtocolError {
    #[error("CCID frame is too short: got {0} bytes")]
    FrameTooShort(usize),
    #[error("CCID payload length mismatch: header says {expected}, frame has {actual}")]
    LengthMismatch { expected: usize, actual: usize },
}

impl CcidCommand {
    pub fn decode(frame: &[u8]) -> Result<Self, CcidProtocolError> {
        if frame.len() < 10 {
            return Err(CcidProtocolError::FrameTooShort(frame.len()));
        }

        let message_type = frame[0];
        let payload_length = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]) as usize;
        let slot = frame[5];
        let seq = frame[6];
        let parameters = [frame[7], frame[8], frame[9]];
        let actual_payload_length = frame.len() - 10;

        if payload_length != actual_payload_length {
            return Err(CcidProtocolError::LengthMismatch {
                expected: payload_length,
                actual: actual_payload_length,
            });
        }

        let payload = frame[10..].to_vec();

        Ok(match message_type {
            PC_TO_RDR_ICC_POWER_ON => Self::IccPowerOn {
                slot,
                seq,
                power_select: parameters[0],
            },
            PC_TO_RDR_ICC_POWER_OFF => Self::IccPowerOff { slot, seq },
            PC_TO_RDR_GET_SLOT_STATUS => Self::GetSlotStatus { slot, seq },
            PC_TO_RDR_XFR_BLOCK => Self::XfrBlock {
                slot,
                seq,
                bwi: parameters[0],
                level_parameter: u16::from_le_bytes([parameters[1], parameters[2]]),
                payload,
            },
            _ => Self::Unknown {
                message_type,
                slot,
                seq,
                parameters,
                payload,
            },
        })
    }
}

impl CcidResponse {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::DataBlock {
                slot,
                seq,
                status,
                error,
                chain_parameter,
                payload,
            } => {
                let mut frame = header(
                    RDR_TO_PC_DATA_BLOCK,
                    payload.len(),
                    *slot,
                    *seq,
                    [status.encode(), *error, *chain_parameter],
                );
                frame.extend_from_slice(payload);
                frame
            }
            Self::SlotStatus {
                slot,
                seq,
                status,
                error,
                clock_status,
            } => header(
                RDR_TO_PC_SLOT_STATUS,
                0,
                *slot,
                *seq,
                [status.encode(), *error, *clock_status],
            ),
        }
    }
}

fn header(message_type: u8, payload_length: usize, slot: u8, seq: u8, params: [u8; 3]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(10 + payload_length);
    frame.push(message_type);
    frame.extend_from_slice(&(payload_length as u32).to_le_bytes());
    frame.push(slot);
    frame.push(seq);
    frame.extend_from_slice(&params);
    frame
}

#[cfg(test)]
mod tests {
    use super::{
        CcidCommand, CcidResponse, CommandStatus, IccStatus, PC_TO_RDR_XFR_BLOCK,
        RDR_TO_PC_DATA_BLOCK, SlotStatus,
    };

    #[test]
    fn decode_xfr_block_command() {
        let frame = [
            PC_TO_RDR_XFR_BLOCK,
            0x04,
            0x00,
            0x00,
            0x00,
            0x00,
            0x22,
            0x00,
            0x34,
            0x12,
            0x00,
            0xa4,
            0x04,
            0x00,
        ];

        let decoded = CcidCommand::decode(&frame).expect("decode should succeed");
        assert_eq!(
            decoded,
            CcidCommand::XfrBlock {
                slot: 0,
                seq: 0x22,
                bwi: 0,
                level_parameter: 0x1234,
                payload: vec![0x00, 0xa4, 0x04, 0x00],
            }
        );
    }

    #[test]
    fn encode_data_block_response() {
        let frame = CcidResponse::DataBlock {
            slot: 0,
            seq: 1,
            status: SlotStatus {
                icc: IccStatus::Active,
                command: CommandStatus::NoError,
            },
            error: 0,
            chain_parameter: 0,
            payload: vec![0x90, 0x00],
        }
        .encode();

        assert_eq!(frame[0], RDR_TO_PC_DATA_BLOCK);
        assert_eq!(&frame[1..5], &[0x02, 0x00, 0x00, 0x00]);
        assert_eq!(&frame[10..], &[0x90, 0x00]);
    }
}
