# 🦀 LANdrop

**LANdrop** is a local-first file sharing application for devices on the
same network. No cloud, no account, no upload to a third party — files
move directly between your devices over your own Wi-Fi/LAN.

```text
$ cargo run
╭──────────────────────────────────────────╮
│              🦀 LANdrop                   │
╰──────────────────────────────────────────╯

  Device       my-laptop
  Status       ● Running
  Address      http://192.168.1.25:8080
  Directory    ./received
  Discovery    Enabled
  PIN          Disabled
  Max file     4096 MB

  Scan the QR code in the dashboard to connect from another device.

  Waiting for devices...
```

## Screenshots

> _Add screenshots of the dashboard, upload progress, and incoming-file
> prompt here before publishing._

## Features

- 📶 **Local-first** — files never leave your network; nothing is
  uploaded to any cloud service.
- 📱 **QR-code connect** — scan to open the dashboard from a phone.
- 🖱️ **Drag & drop** uploads with live progress, speed, and ETA.
- 📥 **Accept / reject** prompts for incoming files — nothing is saved to
  disk without explicit approval, and existing files are never
  overwritten.
- 🔄 **Real-time updates** over WebSocket — no polling, no page refresh.
- 🔍 **LAN discovery** via UDP broadcast, with manual IP/QR connection as
  a fallback when broadcast traffic is blocked.
- 🔐 **Optional PIN protection**, filename sanitization, and path
  traversal protection.
- 🌗 **Dark / light mode**, responsive layout, keyboard accessible.
- 🦀 Written in idiomatic, async Rust with **no `unwrap()` in production
  code paths** and a real test suite.

## Architecture

```text
landrop/
├── Cargo.toml
├── src/
│   ├── main.rs        # CLI bootstrap: parses args, starts the server
│   ├── lib.rs          # library root (re-exports everything below)
│   ├── config.rs       # CLI args + resolved runtime configuration
│   ├── server.rs       # AppState + axum Router assembly
│   ├── routes/
│   │   ├── mod.rs
│   │   ├── auth.rs      # PIN verification + session tokens
│   │   ├── status.rs    # /api/status, /api/qr
│   │   ├── devices.rs   # /api/devices
│   │   ├── upload.rs    # streaming multipart upload
│   │   └── download.rs  # download, history, accept/reject, delete
│   ├── websocket.rs     # broadcast channel + /ws handler
│   ├── discovery.rs     # UDP broadcast LAN discovery
│   ├── security.rs      # filename sanitization, path traversal guards
│   ├── storage.rs       # in-memory + JSON-persisted transfer history
│   └── models.rs        # shared request/response/domain types
├── web/
│   ├── index.html
│   ├── style.css
│   └── app.js           # vanilla JS — no framework, no build step
└── tests/
    └── api_integration.rs
```

The binary is a thin wrapper around a library crate (`landrop`). This
means the integration tests build the *real* `axum::Router` in-process
(via `tower::ServiceExt::oneshot`) instead of needing a live TCP socket.

### Request flow

1. A browser loads `/`, which serves `web/index.html`. Static assets are
   served from `web/` under `/static/*`.
2. The dashboard calls `GET /api/status` and `GET /api/qr` to render the
   connection card, `GET /api/devices` and `GET /api/history` to populate
   the rest of the UI, then opens a `GET /ws` WebSocket for live updates.
3. Dragging a file in POSTs a `multipart/form-data` request to
   `POST /api/upload`, streamed directly to a temporary holding file while
   progress events are broadcast over the WebSocket.
4. If the upload came from another device sending *to* you, it lands in
   `Pending` state and shows up as an "Incoming file" card. Accepting it
   (`POST /api/transfers/:id` with `{"action":"accept"}`) moves it into the
   receiving directory (never overwriting an existing file); rejecting it
   deletes the temp file. If you're sharing your *own* file from the
   dashboard, it's stored immediately and becomes downloadable by peers.

## Requirements

- Rust 1.75+ (2021 edition)
- A local network that permits UDP broadcast, if you want automatic
  discovery (entirely optional — manual IP/QR connection always works)

## Installation

```bash
git clone https://github.com/yourname/landrop.git
cd landrop
cargo build --release
```

## Running locally

```bash
cargo run
```

This starts LANdrop on port `8080`, writing received files to
`./received`. Open the printed address, or the same address from another
device on your network, and scan the QR code shown in the dashboard.

## CLI usage

```bash
landrop                          # start with defaults
landrop --port 9000              # custom port
landrop --directory ~/Downloads  # where received files are saved
landrop --pin 123456             # require a PIN to use the dashboard/API
landrop --no-discovery           # disable UDP broadcast discovery
landrop --max-file-size-mb 1024  # cap uploads at 1 GB
landrop --no-history             # don't persist transfer history to disk
landrop --log-level debug        # error | warn | info | debug | trace
landrop --bind 127.0.0.1         # restrict to localhost only
```

## Configuration

| Flag                  | Default     | Description                                   |
|------------------------|-------------|------------------------------------------------|
| `--port`               | `8080`      | HTTP port to listen on                         |
| `--bind`               | `0.0.0.0`   | Interface to bind (all interfaces by default)  |
| `--directory`          | `./received`| Where accepted/shared files are stored         |
| `--pin`                | _(none)_    | 4–8 digit PIN required to use the dashboard    |
| `--no-discovery`       | off         | Disable UDP broadcast discovery                |
| `--max-file-size-mb`   | `4096`      | Maximum accepted upload size, in MB            |
| `--no-history`         | off         | Disable persisting transfer history to disk    |
| `--log-level`          | `info`      | `error`/`warn`/`info`/`debug`/`trace`          |

## Security considerations

LANdrop is built for **trusted local networks** — it is *not* designed to
be exposed to the public internet. Concretely, it implements:

- **Filename sanitization** and **path traversal protection**: every
  filename is sanitized and re-validated against the target directory
  before touching the filesystem (`security::safe_join`), even though a
  sanitized name can't legitimately contain `..` or a separator.
- **No silent overwrites**: if a filename collides with an existing file,
  LANdrop appends `(1)`, `(2)`, etc. rather than overwriting.
- **Optional PIN authentication** with random, in-memory session tokens
  (reset on restart, never written to disk).
- **Configurable maximum file size**, enforced both as an HTTP body limit
  and during streaming (so an oversized upload is aborted early rather
  than filling the disk).
- **Safe temporary files**: incoming files are streamed to a private
  `.landrop/tmp` directory and only moved into the receiving directory
  after explicit accept.
- **No arbitrary filesystem access**: the server only ever reads/writes
  inside the configured receiving directory and its own data directory.

If you expose LANdrop beyond your LAN (e.g. via a VPN or port forward),
set a PIN and understand that current versions do not implement TLS —
traffic is unencrypted HTTP, which is a reasonable trade-off for "phones
on the same Wi-Fi" but not for the open internet.

## API documentation

| Method | Path                     | Auth\* | Description                                  |
|--------|--------------------------|--------|-----------------------------------------------|
| GET    | `/`                      | —      | Dashboard HTML                                |
| GET    | `/api/status`            | —      | Server status (online, address, config)       |
| GET    | `/api/qr`                | —      | QR code (SVG) encoding the connection URL     |
| GET    | `/api/devices`           | ✅     | List of known LAN devices                     |
| POST   | `/api/upload`             | ✅     | Multipart file upload (`file`, optional `sender`, `direction`) |
| GET    | `/api/download/:id`       | ✅     | Stream a completed transfer's file            |
| POST   | `/api/transfers/:id`      | ✅     | `{"action":"accept"|"reject"}` for a pending incoming file |
| DELETE | `/api/transfers/:id`      | ✅     | Remove a transfer from history                |
| GET    | `/api/history`           | ✅     | Full transfer history                         |
| POST   | `/api/auth`               | —      | Exchange a PIN for a session token             |
| WS     | `/ws`                    | —      | Real-time transfer/device events               |

\* Auth is only enforced when `--pin` is set. Send the token from
`/api/auth` in the `X-Landrop-Token` header.

All error responses are JSON: `{"error": "snake_case_code", "message": "human readable"}`.

## Development guide

```bash
cargo fmt
cargo clippy --all-targets --all-features
cargo build
```

The project is split into a library crate (`src/lib.rs`) and a thin
binary (`src/main.rs`) specifically so integration tests can exercise the
real `axum::Router` in-process without opening a socket.

## Testing

```bash
cargo test
```

Covers:

- Filename sanitization & path traversal prevention (`src/security.rs`)
- PIN hashing/verification and token generation (`src/security.rs`)
- CLI/config validation, including PIN format rules (`src/config.rs`)
- Transfer state transitions and history persistence (`src/storage.rs`)
- End-to-end API behavior — status, PIN auth flow, 404s, QR content type
  (`tests/api_integration.rs`)

## Roadmap

- [ ] TLS support (self-signed cert generated on first run) for
  encrypted LAN transfers
- [ ] Direct device-to-device push (send straight to a discovered peer
  without opening their dashboard first)
- [ ] Resumable uploads for very large files / flaky Wi-Fi
- [ ] Folder (multi-file/archive) transfers
- [ ] mDNS/Bonjour-based discovery as an alternative to UDP broadcast

## License

MIT — see [LICENSE](LICENSE).
