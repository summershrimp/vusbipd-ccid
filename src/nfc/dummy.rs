use super::{
    CardPresence, CardProtocol, NfcReader, ReaderCapabilities, ReaderError, ReaderFactory,
};

const CTAP2_ERR_INVALID_COMMAND: u8 = 0x01;
const FIDO_AID: &[u8] = &[0xa0, 0x00, 0x00, 0x06, 0x47, 0x2f, 0x00, 0x01];
const FIDO_NFCCTAP_MSG: u8 = 0x10;
const FIDO_NFCCTAP_GET_RESPONSE: u8 = 0x11;
const FIDO_NFCCTAP_CONTROL: u8 = 0x12;
const FIDO_NFC_END_CTAP_MSG: u8 = 0x01;
const FIDO_VERSION_U2F_V2: &[u8] = b"U2F_V2";
const FIDO_VERSION_2_0: &[u8] = b"FIDO_2_0";
const FIDO_TRANSPORT_NFC: &[u8] = b"nfc";
const FIDO_DUMMY_AAGUID: [u8; 16] = [
    0x76, 0x75, 0x73, 0x62, 0x69, 0x70, 0x64, 0x2d, 0x63, 0x63, 0x69, 0x64, 0x2d, 0x66, 0x69,
    0x64,
];
const SW_SUCCESS: [u8; 2] = [0x90, 0x00];
const SW_INS_NOT_SUPPORTED: [u8; 2] = [0x6d, 0x00];
const SW_WRONG_PARAMETERS: [u8; 2] = [0x6a, 0x86];
const SW_WRONG_LENGTH: [u8; 2] = [0x67, 0x00];
const SW_FILE_NOT_FOUND: [u8; 2] = [0x6a, 0x82];

pub struct DummyReaderFactory;

impl ReaderFactory for DummyReaderFactory {
    fn backend_name(&self) -> &'static str {
        "dummy"
    }

    fn open(&self) -> Result<Box<dyn NfcReader>, ReaderError> {
        Ok(Box::new(DummyReader {
            card_present: true,
            selected_applet: SelectedApplet::None,
        }))
    }
}

struct DummyReader {
    card_present: bool,
    selected_applet: SelectedApplet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectedApplet {
    None,
    Fido,
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
                historical_bytes: FIDO_VERSION_2_0.to_vec(),
            }))
        } else {
            Ok(None)
        }
    }

    fn power_off(&mut self) -> Result<(), ReaderError> {
        self.selected_applet = SelectedApplet::None;
        Ok(())
    }

    fn exchange_apdu(&mut self, apdu: &[u8]) -> Result<Vec<u8>, ReaderError> {
        self.handle_apdu(apdu)
    }
}

impl DummyReader {
    fn handle_apdu(&mut self, apdu: &[u8]) -> Result<Vec<u8>, ReaderError> {
        if !self.card_present {
            return Err(ReaderError::Protocol(
                "no card present in dummy reader".to_string(),
            ));
        }

        let Some((cla, ins, p1, p2, data)) = Self::parse_short_apdu(apdu) else {
            return Ok(SW_WRONG_LENGTH.to_vec());
        };

        let response = match (cla, ins, p1, p2) {
            (0x00, 0xa4, 0x04, 0x00) | (0x00, 0xa4, 0x04, 0x0c) if data == FIDO_AID => {
                self.selected_applet = SelectedApplet::Fido;
                Self::append_status(FIDO_VERSION_U2F_V2.to_vec(), SW_SUCCESS)
            }
            (0x80, FIDO_NFCCTAP_MSG, _, 0x00) if self.selected_applet == SelectedApplet::Fido => {
                self.handle_ctap_msg(p1, data)
            }
            (0x80, FIDO_NFCCTAP_GET_RESPONSE, 0x00, 0x00)
                if self.selected_applet == SelectedApplet::Fido =>
            {
                SW_INS_NOT_SUPPORTED.to_vec()
            }
            (0x80, FIDO_NFCCTAP_CONTROL, FIDO_NFC_END_CTAP_MSG, 0x00)
                if self.selected_applet == SelectedApplet::Fido =>
            {
                self.selected_applet = SelectedApplet::None;
                SW_SUCCESS.to_vec()
            }
            (0x00, 0xa4, 0x04, 0x00) | (0x00, 0xa4, 0x04, 0x0c) => SW_FILE_NOT_FOUND.to_vec(),
            _ => SW_INS_NOT_SUPPORTED.to_vec(),
        };

        Ok(response)
    }

    fn handle_ctap_msg(&mut self, p1: u8, data: &[u8]) -> Vec<u8> {
        if p1 & 0x7f != 0 {
            return SW_WRONG_PARAMETERS.to_vec();
        }

        let Some((&command, payload)) = data.split_first() else {
            return SW_WRONG_LENGTH.to_vec();
        };

        let response = match command {
            0x04 if payload.is_empty() => Self::ctap_success(Self::authenticator_get_info()),
            _ => Self::ctap_error(CTAP2_ERR_INVALID_COMMAND),
        };

        Self::append_status(response, SW_SUCCESS)
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

    fn authenticator_get_info() -> Vec<u8> {
        let mut response = vec![0xa6];

        response.push(0x01);
        response.push(0x82);
        response.extend(Self::encode_text(FIDO_VERSION_U2F_V2));
        response.extend(Self::encode_text(FIDO_VERSION_2_0));

        response.push(0x03);
        response.push(0x50);
        response.extend_from_slice(&FIDO_DUMMY_AAGUID);

        response.push(0x04);
        response.push(0xa4);
        response.extend(Self::encode_text(b"rk"));
        response.push(0xf4);
        response.extend(Self::encode_text(b"up"));
        response.push(0xf5);
        response.extend(Self::encode_text(b"plat"));
        response.push(0xf4);
        response.extend(Self::encode_text(b"clientPin"));
        response.push(0xf4);

        response.push(0x05);
        response.extend_from_slice(&[0x19, 0x04, 0x00]);

        response.push(0x06);
        response.extend_from_slice(&[0x81, 0x01]);

        response.push(0x09);
        response.push(0x81);
        response.extend(Self::encode_text(FIDO_TRANSPORT_NFC));

        response
    }

    fn ctap_success(payload: Vec<u8>) -> Vec<u8> {
        let mut response = Vec::with_capacity(payload.len() + 1);
        response.push(0x00);
        response.extend_from_slice(&payload);
        response
    }

    fn ctap_error(code: u8) -> Vec<u8> {
        vec![code]
    }

    fn encode_text(value: &[u8]) -> Vec<u8> {
        assert!(value.len() < 24, "dummy CBOR helper only supports short text");

        let mut encoded = Vec::with_capacity(value.len() + 1);
        encoded.push(0x60 | value.len() as u8);
        encoded.extend_from_slice(value);
        encoded
    }

    fn append_status(mut response: Vec<u8>, status: [u8; 2]) -> Vec<u8> {
        response.extend_from_slice(&status);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::{DummyReader, FIDO_VERSION_2_0, SelectedApplet};

    #[test]
    fn select_fido_aid_returns_u2f_v2() {
        let mut reader = DummyReader {
            card_present: true,
            selected_applet: SelectedApplet::None,
        };

        let response = reader
            .handle_apdu(&[
                0x00, 0xa4, 0x04, 0x00, 0x08, 0xa0, 0x00, 0x00, 0x06, 0x47, 0x2f, 0x00,
                0x01,
            ])
            .expect("SELECT must succeed");

        assert_eq!(&response[..6], b"U2F_V2");
        assert!(response.ends_with(&[0x90, 0x00]));
    }

    #[test]
    fn authenticator_get_info_returns_success_status() {
        let mut reader = DummyReader {
            card_present: true,
            selected_applet: SelectedApplet::None,
        };

        let _ = reader
            .handle_apdu(&[
                0x00, 0xa4, 0x04, 0x00, 0x08, 0xa0, 0x00, 0x00, 0x06, 0x47, 0x2f, 0x00,
                0x01,
            ])
            .expect("SELECT must succeed");

        let response = reader
            .handle_apdu(&[0x80, 0x10, 0x80, 0x00, 0x01, 0x04])
            .expect("authenticatorGetInfo must succeed");

        assert_eq!(response[0], 0x00);
        assert!(response.windows(FIDO_VERSION_2_0.len()).any(|window| window == FIDO_VERSION_2_0));
        assert!(response.ends_with(&[0x90, 0x00]));
    }
}
