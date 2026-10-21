# Client architecture

```
app/ui (TypeScript, Vite)  ── Tauri IPC ──  app/src-tauri (ow-app)
                                                   │
                                     crates/* (Rust core, shared by all platforms)
```

- **Rust core** (`crates/`): one crate per domain, no UI code. Compiles for desktop, mobile and WASM.
- **`ow-app`**: thin Tauri 2 layer exposing core features as commands/events.
- **UI**: framework still to be chosen (SolidJS or Svelte); the placeholder is plain TypeScript.

| Crate | Role |
|---|---|
| `ow-identity` | Identity: Ed25519 keys, recovery phrase, multi-device |
| `ow-crypto` | End-to-end encryption: Signal Protocol (DMs) and MLS (groups) |
| `ow-p2p` | P2P transport: libp2p, DHT, QUIC, hole punching, relays |
| `ow-messaging` | Direct messages and groups: sync and store-and-forward |
| `ow-files` | P2P file transfer: BLAKE3 chunks, resume, swarm |
| `ow-voice` | Voice: Opus, noise suppression, spatial audio |
| `ow-video` | Video: AV1, VP9, H.264 with hardware encoding |
| `ow-screenshare` | Screen sharing: multi-window capture and per-app audio |
| `ow-plugins` | Plugin runtime: WASM sandbox, UI sandbox, permissions |
| `ow-presence` | Rich presence over local IPC |
| `ow-mumblelink` | MumbleLink shared-memory reader for positional audio |
| `ow-storage` | Encrypted local SQLite storage |
| `ow-search` | Encrypted local search |
| `ow-antispam` | Anti-spam: rate limits and proof of work |

Performance targets: < 150 MB RAM idle, < 1 s startup, low-power mode for old PCs and mobile.
