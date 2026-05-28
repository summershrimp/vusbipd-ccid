use super::{
    CardPresence, CardProtocol, NfcReader, ReaderCapabilities, ReaderError, ReaderFactory,
};

const SW_SUCCESS: [u8; 2] = [0x90, 0x00];
const SW_FILE_NOT_FOUND: [u8; 2] = [0x6a, 0x82];
const SW_INS_NOT_SUPPORTED: [u8; 2] = [0x6d, 0x00];
const SW_WRONG_LENGTH: [u8; 2] = [0x67, 0x00];
const PIV_AID: &[u8] = &[0xa0, 0x00, 0x00, 0x03, 0x08, 0x00, 0x00, 0x10, 0x00, 0x01, 0x00];
const PIV_AID_PREFIX: &[u8] = &[0xa0, 0x00, 0x00, 0x03, 0x08];
const PIV_OBJECT_DISCOVERY: &[u8] = &[
    0x7e, 0x12, 0x4f, 0x0b, 0xa0, 0x00, 0x00, 0x03, 0x08, 0x00, 0x00, 0x10, 0x00, 0x01,
    0x00, 0x5f, 0x2f, 0x02, 0x01, 0x00,
];
const PIV_OBJECT_CHUID: &[u8] = &[
    0x30, 0x19, 0xd4, 0xe7, 0x39, 0xda, 0x73, 0x9c, 0xed, 0x39, 0xce, 0x73, 0x9d, 0x83,
    0x68, 0x58, 0x21, 0x08, 0x42, 0x10, 0x84, 0x21, 0xc8, 0x42, 0x10, 0xc3, 0xeb, 0x34,
    0x08, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a,
    0xbc, 0xde, 0xf0, 0x35, 0x08, 0x32, 0x30, 0x33, 0x30, 0x30, 0x31, 0x30, 0x31, 0x3e,
    0x00, 0xfe, 0x00,
];
const PIV_OBJECT_CCC: &[u8] = &[
    0xf0, 0x15, 0xa0, 0x00, 0x00, 0x01, 0x16, 0xff, 0x02, 0x10, 0x32, 0x54, 0x76, 0x98,
    0xba, 0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xf1, 0x01, 0x21, 0xf2, 0x01,
    0x21, 0xf3, 0x00, 0xf4, 0x01, 0x00, 0xf5, 0x01, 0x10, 0xf6, 0x00, 0xf7, 0x00, 0xfa,
    0x00, 0xfb, 0x00, 0xfc, 0x00, 0xfd, 0x00, 0xfe, 0x00,
];

pub struct DummyReaderFactory;

impl ReaderFactory for DummyReaderFactory {
    fn backend_name(&self) -> &'static str {
        "dummy"
    }

    fn open(&self) -> Result<Box<dyn NfcReader>, ReaderError> {
        Ok(Box::new(DummyReader { card_present: true }))
    }
}

struct DummyReader {
    card_present: bool,
}

impl NfcReader for DummyReader {
    fn capabilities(&self) -> ReaderCapabilities {
        ReaderCapabilities {
            name: "dummy",
            supports_iso_dep: true,
            supports_apdu_exchange: true,
        }
    }

    fn poll_card(&mut self) -> Result<Option<CardPresence>, ReaderError> {
        if self.card_present {
            Ok(Some(CardPresence {
                uid: vec![0x01, 0x02, 0x03, 0x04],
                protocol: CardProtocol::IsoDep,
                historical_bytes: vec![0x4f, 0x0b, 0xa0, 0x00, 0x00, 0x03, 0x08, 0x00],
            }))
        } else {
            Ok(None)
        }
    }

    fn power_off(&mut self) -> Result<(), ReaderError> {
        // self.card_present = false;
        Ok(())
    }

    fn exchange_apdu(&mut self, _apdu: &[u8]) -> Result<Vec<u8>, ReaderError> {
        self.handle_apdu(_apdu)
    }
}

impl DummyReader {
    fn handle_apdu(&self, apdu: &[u8]) -> Result<Vec<u8>, ReaderError> {
        if !self.card_present {
            return Err(ReaderError::Protocol(
                "no card present in dummy reader".to_string(),
            ));
        }

        let Some((cla, ins, p1, p2, data)) = Self::parse_short_apdu(apdu) else {
            return Ok(SW_WRONG_LENGTH.to_vec());
        };

        let response = match (cla, ins, p1, p2) {
            (_, 0xa4, 0x04, 0x00) | (_, 0xa4, 0x04, 0x0c) if Self::is_piv_select(data) => {
                Self::select_piv_response()
            }
            (_, 0xa4, 0x00, 0x00) | (_, 0xa4, 0x00, 0x0c) => SW_SUCCESS.to_vec(),
            (_, 0xcb, 0x3f, 0xff) => Self::get_data_response(data),
            (_, 0x20, 0x00, 0x80) => SW_SUCCESS.to_vec(),
            _ => SW_INS_NOT_SUPPORTED.to_vec(),
        };

        Ok(response)
    }

    fn parse_short_apdu(apdu: &[u8]) -> Option<(u8, u8, u8, u8, &[u8])> {
        if apdu.len() < 4 {
            return None;
        }

        let cla = apdu[0];
        let ins = apdu[1];
        let p1 = apdu[2];
        let p2 = apdu[3];
        let body = &apdu[4..];

        if body.is_empty() {
            return Some((cla, ins, p1, p2, &[]));
        }

        if body.len() == 1 {
            return Some((cla, ins, p1, p2, &[]));
        }

        let lc = body[0] as usize;
        if body.len() < 1 + lc {
            return None;
        }

        Some((cla, ins, p1, p2, &body[1..1 + lc]))
    }

    fn is_piv_select(data: &[u8]) -> bool {
        data == PIV_AID || data == PIV_AID_PREFIX
    }

    fn select_piv_response() -> Vec<u8> {
        let mut response = vec![0x61, 0x11, 0x4f, 0x0b];
        response.extend_from_slice(PIV_AID);
        response.extend_from_slice(&[0x79, 0x02, 0x4f, 0x00]);
        response.extend_from_slice(&SW_SUCCESS);
        response
    }

    fn get_data_response(request: &[u8]) -> Vec<u8> {
        let Some(object_id) = Self::parse_object_id(request) else {
            return SW_FILE_NOT_FOUND.to_vec();
        };

        match object_id {
            [0x7e] => {
                let mut response = PIV_OBJECT_DISCOVERY.to_vec();
                response.extend_from_slice(&SW_SUCCESS);
                response
            }
            [0x5f, 0xc1, 0x02] => Self::wrap_piv_object(PIV_OBJECT_CHUID),
            [0x5f, 0xc1, 0x07] => Self::wrap_piv_object(PIV_OBJECT_CCC),
            _ => SW_FILE_NOT_FOUND.to_vec(),
        }
    }

    fn parse_object_id(request: &[u8]) -> Option<&[u8]> {
        if request.len() < 2 || request[0] != 0x5c {
            return None;
        }

        let len = request[1] as usize;
        if request.len() < 2 + len {
            return None;
        }

        Some(&request[2..2 + len])
    }

    fn wrap_piv_object(object: &[u8]) -> Vec<u8> {
        let mut response = vec![0x53, object.len() as u8];
        response.extend_from_slice(object);
        response.extend_from_slice(&SW_SUCCESS);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::DummyReader;

    #[test]
    fn select_piv_aid_returns_success() {
        let reader = DummyReader { card_present: true };

        let response = reader
            .handle_apdu(&[
                0x00, 0xa4, 0x04, 0x00, 0x0b, 0xa0, 0x00, 0x00, 0x03, 0x08, 0x00, 0x00,
                0x10, 0x00, 0x01, 0x00,
            ])
            .expect("SELECT must succeed");

        assert!(response.ends_with(&[0x90, 0x00]));
    }

    #[test]
    fn get_data_for_chuid_returns_wrapped_object() {
        let reader = DummyReader { card_present: true };

        let response = reader
            .handle_apdu(&[0x00, 0xcb, 0x3f, 0xff, 0x05, 0x5c, 0x03, 0x5f, 0xc1, 0x02])
            .expect("GET DATA must succeed");

        assert_eq!(response[0], 0x53);
        assert!(response.ends_with(&[0x90, 0x00]));
    }
}
