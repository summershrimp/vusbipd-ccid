#[derive(Debug, Clone)]
pub struct DependencyStack {
    pub usbip_crate: &'static str,
    pub pn532_crate: &'static str,
    pub apdu_command_capacity: usize,
    pub apdu_response_capacity: usize,
    pub ctaphid_message_capacity: usize,
    pub has_ctaphid_reference: bool,
}

impl DependencyStack {
    pub fn detect() -> Self {
        let _ = usbd_ccid::Status::Idle;
        let _ = usbd_ctaphid::Version::default();

        Self {
            usbip_crate: "usbip 0.8.0",
            pn532_crate: "pn532 0.5.0",
            apdu_command_capacity: apdu_dispatch::command::SIZE,
            apdu_response_capacity: apdu_dispatch::response::SIZE,
            ctaphid_message_capacity: ctaphid_dispatch::DEFAULT_MESSAGE_SIZE,
            has_ctaphid_reference: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DependencyStack;

    #[test]
    fn exposes_expected_stack_capacities() {
        let stack = DependencyStack::detect();
        assert_eq!(stack.apdu_command_capacity, 7609);
        assert_eq!(stack.apdu_response_capacity, 7609);
        assert_eq!(stack.ctaphid_message_capacity, 7609);
        assert!(stack.has_ctaphid_reference);
    }
}
