# OutpostWave-Client

*Version française : [README.fr.md](README.fr.md).*

**OutpostWave** — *the outpost of the airwaves*. An open-source, privacy-first alternative to Discord
for gamers: peer-to-peer end-to-end encrypted DMs and groups, self-hosted or official community servers,
voice, video, screen sharing and unlimited P2P file transfer.

This repository contains the **client**: the Rust core (`crates/`) and the Tauri 2 app (`app/`)
for Windows, macOS, Linux, Android and iOS (x86_64 and ARM64).

> Status: **early skeleton** — nothing usable yet.

## Prerequisites

- [Rust](https://rustup.rs) (the exact version is pinned in `rust-toolchain.toml`)
- [Node.js](https://nodejs.org) 20+
- Tauri system dependencies: <https://v2.tauri.app/start/prerequisites/>
- **Windows only**: [Strawberry Perl](https://strawberryperl.com) (`winget install StrawberryPerl.StrawberryPerl`), needed to build the OpenSSL bundled with SQLCipher. NASM is optional.

## Develop

```sh
cargo test --workspace          # Rust core
cd app
npm install
npm run tauri dev               # run the desktop app
npm run tauri build             # release bundle
```

## Test a conversation on one PC

Each profile is an independent identity (own data folder and database key):

```sh
cd app && npm run tauri dev              # profile "default"
# in another terminal, from the repository root:
OW_PROFILE=test2 target/debug/ow-app     # Windows: set OW_PROFILE=test2 && target\debug\ow-app.exe
```

Create an account in each window, copy the invite code of one ("My code") into the other
("Add a contact"), then chat. Two PCs on the same local network find each other automatically.

## Layout

| Path | Content |
|---|---|
| `crates/` | Rust core, one crate per domain |
| `app/src-tauri/` | Tauri 2 native layer |
| `app/ui/` | Frontend (components, views, i18n, themes) |
| `web/` | WASM browser build |
| `mobile/` | Android / iOS specifics |
| `docs/`, `tests/` | Documentation, integration tests |

## License

[GPL-3.0](LICENSE): forks of the client must stay free software.
Contributions require accepting the [CLA](CLA.md).
