# Dashbeam Inbox CLI

An upload-only inbox for a Windows receiver and a macOS sender. It reuses Dashbeam's
Rust file-transfer engine without the desktop UI, device pairing, or Trust feature.

## Build

Install Rust 1.91 or later. On Windows, install the Visual Studio C++ build tools;
on macOS, install the Xcode command line tools (`xcode-select --install`).
From the repository root:

```sh
cargo build --locked --release --manifest-path engine/Cargo.toml -p dashbeam-inbox
cargo test --locked --manifest-path engine/Cargo.toml -p dashbeam-inbox
```

The executable is `engine/target/release/dashbeam-inbox.exe` on Windows and
`engine/target/release/dashbeam-inbox` on macOS. Node, pnpm, and Tauri are not needed.
The Inbox CLI workflow builds Windows x64 and macOS Apple Silicon/Intel artifacts.

## Set up the Windows receiver

The Windows executable requires the x64 Microsoft Visual C++ v14 runtime.
If it is not already installed, use the [latest supported x64 redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist).
Rust and Visual Studio are only needed to build, not to run the executable.

Keep configuration in a private directory owned by the account running the receiver.
Run these commands in PowerShell from the folder containing the executable:

```powershell
.\dashbeam-inbox.exe init --config receiver.json --credential sender.json
.\dashbeam-inbox.exe listen --config receiver.json --output 'C:\Inbox'
```

`receiver.json` contains the receiver's private identity key and a hash of the
upload token. Keep this file on the receiver. `sender.json` contains the public
receiver identity and a randomly generated 256-bit upload token. Copy only
`sender.json` to your Mac using a private channel. Credentials are not printed.
On macOS, newly created credential files have owner-only permissions; on Windows,
they inherit the selected directory's permissions.

The listener runs until Ctrl+C. Keep the PC awake and this process running to
accept files at any time. It does not install a Windows service or startup task.
There is no offline cloud queue: both machines must be online during a transfer.
The default network configuration uses Iroh discovery and public relays, with a
direct encrypted connection when possible.

## Send from macOS

```sh
./dashbeam-inbox send --credential sender.json ./report.pdf
./dashbeam-inbox send --credential sender.json ./photos ./report.pdf
```

Reuse the same credential for later transfers, including after restarting the
receiver with the same configuration. Each successful upload lands in a separate
randomly named directory inside `C:\Inbox`. Failed exports are removed from that
inbox; partial download data can remain in the receiver's private temporary
cache until the process exits. No received file is executed or automatically opened.

The timeout defaults to one hour. For a longer transfer, set it on both sides:

```powershell
.\dashbeam-inbox.exe listen --config receiver.json --output 'C:\Inbox' --timeout-seconds 14400
```

```sh
./dashbeam-inbox send --credential sender.json --timeout-seconds 14400 ./large-folder
```

Only one authenticated upload is accepted at a time. Other authenticated senders
receive a busy error and can retry. The receiver remains listening after completion
or rejection.

## Coexisting with the desktop app

The CLI is a separate executable with its own receiver identity and credentials.
It does not change the installed desktop app, its settings, paired devices, or
transfer history. Each CLI process uses a private temporary directory beneath
the system temp folder, outside the desktop app's startup cleanup targets.
This also separates simultaneous CLI processes. Temporary caches are removed
on normal exit; an abrupt process termination can leave a cache behind.

The stock desktop app cannot send to the token-protected CLI inbox. Use the CLI
on both ends for inbox transfers, and keep the desktop app for ordinary sharing.

## Direction and authorization

- The token grants permission to upload into this inbox only.
- The receiver does not register file-download, metadata, pairing, or presence
  protocols. Its inbox protocol has no list, download, or reverse-send operation.
- The sender authenticates the receiver's public identity before transmitting the
  token, then waits for authorization before preparing the files.
- Files move from sender to receiver. Acknowledgements, status, and download
  requests still travel in both directions; this is not a one-way network link.
- A credential does not give the receiver permission to push files to the sender.
  Someone who controls either computer can still run other software; this CLI
  does not impose operating-system restrictions.
- Anyone holding `sender.json` can upload. Treat it as a password. There is no
  per-sender allowlist, antivirus scan, or disk quota in this first version.
- The stock Dashbeam desktop app does not speak this new inbox protocol. Use this
  CLI on both ends.

To revoke a credential, stop the receiver, run `init` with two new filenames, then
restart `listen` with the new receiver configuration. This replaces both the
receiver identity and its token. Distribute the new sender credential. Existing
configuration files are never overwritten by `init`.

## Checks

The test suite covers valid and invalid tokens, preserving the receiver identity,
refusing configuration overwrite, bounded control messages, unsupported read/list
requests, repeated nested-file uploads, rejection before local file access,
unauthenticated upload attempts, and refusal of file-download/pairing protocols.
Transfer tests use real local encrypted connections and compare received bytes.
They do not establish cross-platform or internet connectivity by themselves.

The application uses the repository's AGPL-3.0 license; see `LICENSE` at the root.
