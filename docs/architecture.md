# Architecture Notes

## Problem Statement

We need a server-side USB/IP implementation that exports a virtual CCID reader instead of forwarding a physical USB reader. Internally, the exported CCID reader should use an NFC reader backend to detect and communicate with contactless security tokens such as a YubiKey over NFC.

## Design Goals

- Cross-platform runtime target: Windows and Linux.
- Implementation language: Rust.
- First reader backend: PN532 over UART.
- The code must be ready for additional reader backends later.
- Keep USB/IP, CCID, and reader-specific logic separated.
- Reuse existing Rust crates where possible to avoid reimplementing mature protocol pieces.

## Selected Crate Strategy

The current direction is to lean on these crates:

- `usbip 0.8.0` for the USB/IP server and simulated USB device model.
- `pn532` for PN532 command transport and response parsing.
- `usbd-ccid` plus `apdu-dispatch` as the main CCID/APDU reference stack.
- `usbd-ctaphid` plus `ctaphid-dispatch` as a future FIDO HID path if exporting CTAPHID becomes necessary.

Important caveat: the `usbd-*` crates are designed around `usb-device` and embedded USB buses. They cannot be plugged directly into a host-side USB/IP server. For this project they are mainly valuable as:

- protocol references,
- behavior references,
- message sizing references,
- future adapter targets if we build a compatibility layer.

## Layered Structure

### 1. USB/IP layer

Responsibilities:

- own TCP listening and connection lifecycle,
- reuse the `usbip` crate's server and simulated device model,
- route bulk/control traffic to the CCID device model.

This layer should not know anything about PN532.

### 2. CCID layer

Responsibilities:

- decode CCID commands from the virtual USB device,
- map CCID slot state to NFC token state,
- expose CCID responses,
- translate APDU exchange requests into backend reader operations.

This layer should not depend on a specific NFC reader implementation.

### 3. NFC layer

Responsibilities:

- abstract reader discovery / opening / configuration,
- detect card presence,
- exchange APDUs with an NFC token,
- translate backend-specific protocols into a common reader API,
- use crate-backed protocol implementations where possible instead of custom frame parsing.

Reader-specific code, such as PN532 framing or UART transport, belongs here.

## Abstraction Strategy

The key extension point is the `NfcReader` trait.

This trait is intentionally focused on capabilities that the CCID layer needs instead of exposing PN532-specific operations. That should allow us to add new backends later, for example:

- PN532 over UART,
- PC/SC-backed desktop readers,
- ACR122U-like readers,
- PN7150/PN7160-style controllers,
- vendor SDK based readers.

## Planned Runtime Flow

1. Parse CLI/config.
2. Open the configured NFC backend.
3. Build a virtual CCID USB device on top of the `usbip` crate.
4. Start the USB/IP listener.
4. When the client issues CCID commands:
   - `IccPowerOn` triggers card detection and returns a pseudo ATR.
   - `GetSlotStatus` reports card presence.
   - `XfrBlock` forwards APDU payloads to the NFC backend.
5. The NFC backend talks to the physical reader and returns APDU responses.

## Why The Additional CTAPHID Crates Were Added

The mainline path is still CCID-over-USB/IP. However, FIDO tokens are not always best represented through CCID only. Adding `usbd-ctaphid` and `ctaphid-dispatch` now gives us a ready reference path for a future alternative USB personality that may better match WebAuthn/FIDO expectations on some hosts.

## Notes on YubiKey over NFC

At this stage the design assumes the NFC backend will eventually provide ISO14443-4 / ISO-DEP style APDU exchange against the token. The CCID layer should remain focused on APDU-level semantics and avoid embedding reader-specific framing details.

## Why Start With a Scaffold

Implementing USB/IP device export, CCID emulation, PN532 transport, and NFC token handling all at once is high-risk. The current scaffold reduces that risk by establishing stable boundaries first, so future work can be added with lower coupling.
