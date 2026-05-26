# vusbipd-ccid

`vusbipd-ccid` is a Rust-based USB/IP device exporter that is intended to present a virtual USB CCID smart card reader to a USB/IP client, while bridging CCID APDU traffic to a physical NFC reader.

## Goal

The primary target scenario is:

- The server runs on Windows or Linux.
- The server exports a virtual CCID reader over USB/IP.
- The CCID reader is backed by an NFC reader instead of a real USB CCID device.
- A YubiKey exposed over NFC can then be used by the client through standard CCID/WebAuthn/FIDO flows.

## Initial Scope

- USB/IP server-side device export scaffold.
- Virtual CCID protocol bridge scaffold.
- NFC backend abstraction for future reader support.
- First backend target: PN532 over UART.

## Current Status

This repository currently contains the initial architecture and Rust scaffold:

- a layered project structure,
- CCID command/response modeling,
- an NFC reader trait that is designed for multiple backends,
- a PN532 UART backend scaffold with PN532 frame codec helpers,
- a USB/IP listener scaffold for future protocol work.

The USB/IP enumeration flow and the full PN532 transport command set are not implemented yet.

## Architecture Overview

The codebase is intentionally split into three main layers:

1. `usbip`: owns network transport and future USB device export logic.
2. `ccid`: owns CCID message parsing and CCID-to-NFC bridging decisions.
3. `nfc`: owns physical reader integration and reader-specific protocol details.

This separation is meant to make it easier to add:

- additional NFC readers,
- alternative transports for the same reader family,
- different virtual smart card device behaviors,
- richer device emulation on top of USB/IP.

## Next Milestones

1. Implement enough USB/IP protocol handling to enumerate a single virtual CCID device.
2. Complete PN532 UART command transport and card polling.
3. Translate CCID power-on / APDU exchange flows to ISO14443-4 card interaction.
4. Add integration tests against recorded USB/IP and PN532 traces.

See `docs/architecture.md` for more detail.
