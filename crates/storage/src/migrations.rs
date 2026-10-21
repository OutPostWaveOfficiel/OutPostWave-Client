//! Schema migrations, tracked with `PRAGMA user_version`.
//!
//! Never edit a released migration: append a new one.

/// Ordered migrations. Index `i` upgrades the schema from version `i` to `i + 1`.
pub(crate) const MIGRATIONS: &[&str] = &[
    // v1: identity, contacts, conversations, messages, crypto state.
    r"
    CREATE TABLE local_identity (
        id              INTEGER PRIMARY KEY CHECK (id = 1),
        handle_name     TEXT NOT NULL,
        device_secret   BLOB NOT NULL CHECK (length(device_secret) = 32),
        device_cert     BLOB NOT NULL,
        master_secret   BLOB CHECK (master_secret IS NULL OR length(master_secret) = 32),
        created_at      INTEGER NOT NULL
    ) STRICT;

    CREATE TABLE contacts (
        id              INTEGER PRIMARY KEY,
        master_pub      BLOB NOT NULL UNIQUE CHECK (length(master_pub) = 32),
        handle          TEXT NOT NULL,
        display_name    TEXT,
        verified        INTEGER NOT NULL DEFAULT 0 CHECK (verified IN (0, 1)),
        added_at        INTEGER NOT NULL
    ) STRICT;

    CREATE TABLE contact_devices (
        device_pub      BLOB PRIMARY KEY CHECK (length(device_pub) = 32),
        contact_id      INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
        certificate     BLOB NOT NULL
    ) STRICT;
    CREATE INDEX contact_devices_contact ON contact_devices(contact_id);

    CREATE TABLE conversations (
        id              INTEGER PRIMARY KEY,
        contact_id      INTEGER UNIQUE REFERENCES contacts(id) ON DELETE CASCADE,
        created_at      INTEGER NOT NULL
    ) STRICT;

    CREATE TABLE messages (
        id              INTEGER PRIMARY KEY,
        conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        msg_id          BLOB NOT NULL UNIQUE CHECK (length(msg_id) = 16),
        sender_device   BLOB NOT NULL CHECK (length(sender_device) = 32),
        outgoing        INTEGER NOT NULL CHECK (outgoing IN (0, 1)),
        sent_at         INTEGER NOT NULL,
        received_at     INTEGER,
        status          INTEGER NOT NULL,
        body            TEXT NOT NULL
    ) STRICT;
    CREATE INDEX messages_by_conversation ON messages(conversation_id, sent_at, id);

    -- Serialized cryptographic state (e.g. vodozemac account and sessions), keyed by name.
    CREATE TABLE secrets (
        name            TEXT PRIMARY KEY,
        value           BLOB NOT NULL,
        updated_at      INTEGER NOT NULL
    ) STRICT;
    ",
    // v2: file transfers.
    r"
    ALTER TABLE messages ADD COLUMN file_id BLOB CHECK (file_id IS NULL OR length(file_id) = 32);

    CREATE TABLE files (
        file_id         BLOB PRIMARY KEY CHECK (length(file_id) = 32),
        conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        outgoing        INTEGER NOT NULL CHECK (outgoing IN (0, 1)),
        name            TEXT NOT NULL,
        size            INTEGER NOT NULL,
        file_key        BLOB NOT NULL CHECK (length(file_key) = 32),
        source_device   BLOB NOT NULL CHECK (length(source_device) = 32),
        manifest        BLOB,
        path            TEXT,
        bitmap          BLOB,
        state           INTEGER NOT NULL,
        created_at      INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX files_by_state ON files(state);
    ",
];

/// Latest schema version.
pub(crate) fn latest() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}
