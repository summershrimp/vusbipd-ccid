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
- a USB/IP server wrapper built on top of `usbip 0.8.0`,
- a PN532 UART backend using the `pn532` crate,
- dependency hooks for `usbd-ccid`, `apdu-dispatch`, `usbd-ctaphid`, and `ctaphid-dispatch`.

The project now leans on external crates to reduce protocol implementation effort:

- `usbip 0.8.0` for USB/IP server/device simulation,
- `pn532` for PN532 frame transport and command handling,
- `usbd-ccid` + `apdu-dispatch` as the reference CCID/APDU stack,
- `usbd-ctaphid` + `ctaphid-dispatch` as a future HID/FIDO path if direct CTAPHID export is needed.

The embedded USB class crates are not drop-in replacements for a host-side USB/IP exporter, so they are currently used as protocol references and integration anchors rather than as directly instantiated runtime USB classes.

## Architecture Overview

The codebase is intentionally split into three main layers:

1. `usbip`: owns network transport and future USB device export logic.
2. `ccid`: owns CCID message parsing and CCID-to-NFC bridging decisions.
3. `nfc`: owns physical reader integration and reader-specific protocol details.
4. `stack`: records the intended third-party protocol stack and future CTAPHID/APDU integration points.

This separation is meant to make it easier to add:

- additional NFC readers,
- alternative transports for the same reader family,
- different virtual smart card device behaviors,
- richer device emulation on top of USB/IP.

## Next Milestones

1. Implement enough USB/IP protocol handling to enumerate a single virtual CCID device.
2. Harden the virtual CCID interface so it matches host CCID expectations more closely.
3. Complete PN532 polling and APDU exchange validation against real YubiKey NFC flows.
4. Evaluate whether a CTAPHID-based virtual USB path should be added alongside CCID.
5. Add integration tests against recorded USB/IP and PN532 traces.

See `docs/architecture.md` for more detail.
