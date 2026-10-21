# OutpostWave-Client

**OutpostWave** — *l'avant-poste des ondes*. Alternative open-source à Discord, orientée gamers et
respectueuse de la vie privée : MP et groupes P2P chiffrés de bout en bout, serveurs communautaires
auto-hébergés ou officiels, vocal, vidéo, partage d'écran et transfert de fichiers P2P sans limite.

Ce dépôt contient le **client** : le cœur Rust (`crates/`) et l'application Tauri 2 (`app/`)
pour Windows, macOS, Linux, Android et iOS (x86_64 et ARM64).

> État : **squelette** — rien d'utilisable pour l'instant.

## Prérequis

- [Rust](https://rustup.rs) (version figée dans `rust-toolchain.toml`)
- [Node.js](https://nodejs.org) 20+
- Dépendances système de Tauri : <https://v2.tauri.app/start/prerequisites/>

## Développement

```sh
cargo test --workspace
cd app
npm install
npm run tauri dev
```

## Licence

[GPL-3.0](LICENSE) : les forks du client restent libres.
Les contributions nécessitent l'acceptation du [CLA](CLA.md).
