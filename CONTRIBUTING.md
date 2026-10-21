# Contributing to OutpostWave-Client

*Version française plus bas.*

## English

Thanks for your interest! Before contributing:

1. **Sign the CLA** — read [CLA.md](CLA.md). It is mandatory for external contributors.
2. **Open an issue first** for any non-trivial change, so the design can be discussed.
3. **Code style**
   - Rust: `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` must pass.
   - Code comments and identifiers are in **English**.
   - No `unsafe` unless strictly needed (FFI); every `unsafe` block needs a `// SAFETY:` comment.
4. **Tests**: `cargo test --workspace` must pass. Add tests with every change.
5. **Commits**: small, focused, imperative messages (`Add chunk bitmap exchange`).
6. **Security issues**: never open a public issue. See [SECURITY.md](SECURITY.md).
7. **UI strings** go through i18n (`app/ui/src/i18n`), in both `en.json` and `fr.json`.

Priorities, in order: **performance, compatibility (every OS / every PC), security, privacy.**

## Français

Merci de votre intérêt ! Avant de contribuer :

1. **Signez le CLA** — lisez [CLA.md](CLA.md). Il est obligatoire pour les contributeurs externes.
2. **Ouvrez d'abord une issue** pour tout changement non trivial, afin d'en discuter la conception.
3. **Style de code**
   - Rust : `cargo fmt --all` et `cargo clippy --workspace --all-targets -- -D warnings` doivent passer.
   - Les commentaires et identifiants du code sont en **anglais**.
   - Pas d'`unsafe` sauf nécessité (FFI) ; chaque bloc `unsafe` a un commentaire `// SAFETY:`.
4. **Tests** : `cargo test --workspace` doit passer. Ajoutez des tests à chaque changement.
5. **Commits** : petits, ciblés, messages à l'impératif en anglais.
6. **Failles de sécurité** : jamais d'issue publique. Voir [SECURITY.md](SECURITY.md).
7. **Les textes de l'interface** passent par l'i18n (`app/ui/src/i18n`), dans `en.json` et `fr.json`.

Priorités, dans l'ordre : **performance, compatibilité (tous OS / tous PC), sécurité, vie privée.**
