//! Database access.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use zeroize::{Zeroize, Zeroizing};

use crate::migrations::{MIGRATIONS, latest};
use crate::{
    Contact, ContactId, ConversationId, DbKey, FileRow, FileState, LocalIdentity, MessageStatus,
    NewContact, NewMessage, StorageError, StoredMessage,
};

type Result<T> = core::result::Result<T, StorageError>;

/// Encrypted database handle. Not `Sync`: wrap it in a mutex to share it between threads.
#[derive(Debug)]
pub struct Storage {
    conn: Connection,
}

impl Storage {
    /// Opens (or creates) the encrypted database at `path` and applies pending migrations.
    ///
    /// # Errors
    ///
    /// [`StorageError::WrongKeyOrCorrupted`] if `key` does not match, [`StorageError::TooNew`]
    /// if the schema is newer than this build, or an SQLite error.
    pub fn open(path: &Path, key: &DbKey) -> Result<Self> {
        Self::init(Connection::open(path)?, key)
    }

    /// Opens an encrypted in-memory database (tests, ephemeral sessions).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn open_in_memory(key: &DbKey) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, key)
    }

    fn init(conn: Connection, key: &DbKey) -> Result<Self> {
        let mut pragma = key.pragma_value();
        let keyed = conn.pragma_update(None, "key", &pragma);
        pragma.zeroize();
        keyed?;
        // Fails with NotADatabase if the key is wrong.
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA secure_delete = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let current: u32 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current > latest() {
            return Err(StorageError::TooNew {
                found: current,
                supported: latest(),
            });
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(
                None,
                "user_version",
                i64::try_from(i + 1).unwrap_or(i64::MAX),
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Current schema version.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// SQLCipher version (proves the database is encrypted).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn cipher_version(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row("PRAGMA cipher_version", [], |r| r.get(0))?)
    }

    // --- local identity -------------------------------------------------------------------

    /// Saves (or replaces) this device's identity.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn save_local_identity(&self, id: &LocalIdentity) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO local_identity
                 (id, handle_name, device_secret, device_cert, master_secret, created_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![
                id.handle_name,
                &id.device_secret[..],
                id.device_cert,
                id.master_secret.as_ref().map(|s| &s[..]),
                id.created_at
            ],
        )?;
        Ok(())
    }

    /// Loads this device's identity, if one was created.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn load_local_identity(&self) -> Result<Option<LocalIdentity>> {
        let row = self
            .conn
            .query_row(
                "SELECT handle_name, device_secret, device_cert, master_secret, created_at
                 FROM local_identity WHERE id = 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        Zeroizing::new(r.get::<_, Vec<u8>>(1)?),
                        r.get::<_, Vec<u8>>(2)?,
                        r.get::<_, Option<Vec<u8>>>(3)?.map(Zeroizing::new),
                        r.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((handle_name, device_secret, device_cert, master_secret, created_at)) = row else {
            return Ok(None);
        };
        Ok(Some(LocalIdentity {
            handle_name,
            device_secret: to_array(&device_secret, "device secret")?,
            device_cert,
            master_secret: master_secret
                .map(|m| to_array(&m, "master secret"))
                .transpose()?,
            created_at,
        }))
    }

    // --- contacts -------------------------------------------------------------------------

    /// Adds a contact, or updates its handle if the master key is already known.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn upsert_contact(&self, c: &NewContact) -> Result<ContactId> {
        let id = self.conn.query_row(
            "INSERT INTO contacts (master_pub, handle, display_name, added_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (master_pub) DO UPDATE SET handle = excluded.handle
             RETURNING id",
            params![&c.master_pub[..], c.handle, c.display_name, c.added_at],
            |r| r.get(0),
        )?;
        Ok(ContactId(id))
    }

    /// Lists contacts, sorted by handle.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn contacts(&self) -> Result<Vec<Contact>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, master_pub, handle, display_name, verified, added_at
             FROM contacts ORDER BY handle",
        )?;
        let rows = stmt.query_map([], contact_from_row)?;
        rows.map(|r| r?).collect()
    }

    /// Finds a contact by master public key.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn contact_by_master(&self, master_pub: &[u8; 32]) -> Result<Option<Contact>> {
        self.conn
            .query_row(
                "SELECT id, master_pub, handle, display_name, verified, added_at
                 FROM contacts WHERE master_pub = ?1",
                [&master_pub[..]],
                contact_from_row,
            )
            .optional()?
            .transpose()
    }

    /// Marks a contact's safety number as verified (or not).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn set_contact_verified(&self, id: ContactId, verified: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE contacts SET verified = ?1 WHERE id = ?2",
            params![verified, id.0],
        )?;
        Ok(())
    }

    /// Deletes a contact and its conversation.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn delete_contact(&self, id: ContactId) -> Result<()> {
        self.conn
            .execute("DELETE FROM contacts WHERE id = ?1", [id.0])?;
        Ok(())
    }

    /// Records a (verified) device certificate of a contact.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn add_contact_device(
        &self,
        contact: ContactId,
        device_pub: &[u8; 32],
        certificate: &[u8],
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO contact_devices (device_pub, contact_id, certificate) VALUES (?1, ?2, ?3)
             ON CONFLICT (device_pub) DO UPDATE SET certificate = excluded.certificate",
            params![&device_pub[..], contact.0, certificate],
        )?;
        Ok(())
    }

    /// Certificate of a contact device.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn contact_device_cert(&self, device_pub: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT certificate FROM contact_devices WHERE device_pub = ?1",
                [&device_pub[..]],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Device public keys of a contact.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn contact_devices(&self, contact: ContactId) -> Result<Vec<[u8; 32]>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT device_pub FROM contact_devices WHERE contact_id = ?1")?;
        let rows = stmt.query_map([contact.0], |r| r.get::<_, Vec<u8>>(0))?;
        rows.map(|r| to_array(&r?, "device key")).collect()
    }

    /// Finds the contact owning a device key.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn contact_by_device(&self, device_pub: &[u8; 32]) -> Result<Option<Contact>> {
        self.conn
            .query_row(
                "SELECT c.id, c.master_pub, c.handle, c.display_name, c.verified, c.added_at
                 FROM contacts c JOIN contact_devices d ON d.contact_id = c.id
                 WHERE d.device_pub = ?1",
                [&device_pub[..]],
                contact_from_row,
            )
            .optional()?
            .transpose()
    }

    /// Finds a contact by row id.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn contact(&self, id: ContactId) -> Result<Option<Contact>> {
        self.conn
            .query_row(
                "SELECT id, master_pub, handle, display_name, verified, added_at
                 FROM contacts WHERE id = ?1",
                [id.0],
                contact_from_row,
            )
            .optional()?
            .transpose()
    }

    /// Merges contact `src` into `dst` after a verified key change: `dst` takes the identity
    /// (`new_master`) and devices of `src`, its old devices are removed, the conversations are
    /// joined, `src` is deleted and `dst` must be verified again.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn merge_contact(
        &self,
        src: ContactId,
        dst: ContactId,
        new_master: &[u8; 32],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let conv = |id: ContactId| -> Result<Option<i64>> {
            Ok(tx
                .query_row(
                    "SELECT id FROM conversations WHERE contact_id = ?1",
                    [id.0],
                    |r| r.get(0),
                )
                .optional()?)
        };
        match (conv(src)?, conv(dst)?) {
            (Some(from), Some(to)) => {
                tx.execute(
                    "UPDATE messages SET conversation_id = ?2 WHERE conversation_id = ?1",
                    params![from, to],
                )?;
            }
            (Some(from), None) => {
                tx.execute(
                    "UPDATE conversations SET contact_id = ?2 WHERE id = ?1",
                    params![from, dst.0],
                )?;
            }
            _ => {}
        }
        tx.execute("DELETE FROM contact_devices WHERE contact_id = ?1", [dst.0])?;
        tx.execute(
            "UPDATE contact_devices SET contact_id = ?2 WHERE contact_id = ?1",
            params![src.0, dst.0],
        )?;
        tx.execute("DELETE FROM contacts WHERE id = ?1", [src.0])?;
        tx.execute(
            "UPDATE contacts SET master_pub = ?2, verified = 0 WHERE id = ?1",
            params![dst.0, &new_master[..]],
        )?;
        tx.commit()?;
        Ok(())
    }

    // --- conversations and messages -------------------------------------------------------

    /// Returns the direct-message conversation with `contact`, creating it if needed.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn dm_conversation(&self, contact: ContactId, now: i64) -> Result<ConversationId> {
        let id = self.conn.query_row(
            "INSERT INTO conversations (contact_id, created_at) VALUES (?1, ?2)
             ON CONFLICT (contact_id) DO UPDATE SET contact_id = contact_id
             RETURNING id",
            params![contact.0, now],
            |r| r.get(0),
        )?;
        Ok(ConversationId(id))
    }

    /// Inserts a message. Idempotent on `msg_id`: returns `None` if it was already stored
    /// (duplicate delivery).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn insert_message(&self, m: &NewMessage) -> Result<Option<i64>> {
        let id = self
            .conn
            .query_row(
                "INSERT INTO messages (conversation_id, msg_id, sender_device, outgoing, sent_at,
                                       received_at, status, body, file_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (msg_id) DO NOTHING
                 RETURNING id",
                params![
                    m.conversation.0,
                    &m.msg_id[..],
                    &m.sender_device[..],
                    m.outgoing,
                    m.sent_at,
                    m.received_at,
                    m.status as i64,
                    m.body,
                    m.file_id.as_ref().map(<[u8; 32]>::as_slice)
                ],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id)
    }

    /// Messages of a conversation, newest first, before row id `before` (for pagination).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn messages(
        &self,
        conversation: ConversationId,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, conversation_id, msg_id, sender_device, outgoing, sent_at, received_at,
                    status, body, file_id, edited_at
             FROM messages
             WHERE conversation_id = ?1 AND id < ?2 AND deleted = 0
             ORDER BY sent_at DESC, id DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![conversation.0, before.unwrap_or(i64::MAX), limit],
            message_from_row,
        )?;
        rows.map(|r| r?).collect()
    }

    /// All messages of a contact, newest first, at most `limit` (for backups).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn contact_messages(&self, contact: ContactId, limit: u32) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, m.conversation_id, m.msg_id, m.sender_device, m.outgoing, m.sent_at,
                    m.received_at, m.status, m.body, m.file_id, m.edited_at
             FROM messages m JOIN conversations c ON c.id = m.conversation_id
             WHERE c.contact_id = ?1 AND m.deleted = 0
             ORDER BY m.sent_at DESC, m.id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![contact.0, limit], message_from_row)?;
        rows.map(|r| r?).collect()
    }

    /// Outgoing messages of a conversation not yet delivered, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn undelivered(&self, conversation: ConversationId) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, conversation_id, msg_id, sender_device, outgoing, sent_at, received_at,
                    status, body, file_id, edited_at
             FROM messages
             WHERE conversation_id = ?1 AND outgoing = 1 AND status < 2 AND deleted = 0
             ORDER BY sent_at, id",
        )?;
        let rows = stmt.query_map([conversation.0], message_from_row)?;
        rows.map(|r| r?).collect()
    }

    // --- files -----------------------------------------------------------------------------

    /// Inserts a file transfer (ignored if the id exists).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn insert_file(&self, f: &FileRow) -> Result<()> {
        self.conn.execute(
            &format!(
                "INSERT INTO files ({FILE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT (file_id) DO NOTHING"
            ),
            params![
                &f.file_id[..],
                f.conversation.0,
                f.outgoing,
                f.name,
                i64::try_from(f.size).unwrap_or(i64::MAX),
                &f.key[..],
                &f.source_device[..],
                f.manifest,
                f.path,
                f.bitmap,
                f.state as i64,
                f.created_at
            ],
        )?;
        Ok(())
    }

    /// A file transfer.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn file(&self, file_id: &[u8; 32]) -> Result<Option<FileRow>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {FILE_COLUMNS} FROM files WHERE file_id = ?1"
        ))?;
        stmt.query_row([&file_id[..]], file_from_row)
            .optional()?
            .transpose()
    }

    /// File transfers in one of `states`.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn files_in_state(&self, states: &[FileState]) -> Result<Vec<FileRow>> {
        let list = states
            .iter()
            .map(|s| (*s as i64).to_string())
            .collect::<Vec<_>>()
            .join(",");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM files WHERE state IN ({list})"
        ))?;
        let rows = stmt.query_map([], file_from_row)?;
        rows.map(|r| r?).collect()
    }

    /// Updates the state, path, manifest and bitmap of a file (`None` keeps the current value).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn update_file(
        &self,
        file_id: &[u8; 32],
        state: Option<FileState>,
        path: Option<&str>,
        manifest: Option<&[u8]>,
        bitmap: Option<&[u8]>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET state = COALESCE(?2, state), path = COALESCE(?3, path),
                              manifest = COALESCE(?4, manifest), bitmap = COALESCE(?5, bitmap)
             WHERE file_id = ?1",
            params![
                &file_id[..],
                state.map(|s| s as i64),
                path,
                manifest,
                bitmap
            ],
        )?;
        Ok(())
    }

    /// Contact of a conversation (`None` for group conversations).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn conversation_contact(&self, conversation: ConversationId) -> Result<Option<ContactId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT contact_id FROM conversations WHERE id = ?1",
                [conversation.0],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten()
            .map(ContactId))
    }

    /// Marks incoming messages of a conversation as read locally and returns their ids
    /// (to send read receipts).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn mark_incoming_read(&self, conversation: ConversationId) -> Result<Vec<[u8; 16]>> {
        let mut stmt = self.conn.prepare_cached(
            "UPDATE messages SET status = 3
             WHERE conversation_id = ?1 AND outgoing = 0 AND status = 4
             RETURNING msg_id",
        )?;
        let rows = stmt.query_map([conversation.0], |r| r.get::<_, Vec<u8>>(0))?;
        rows.map(|r| to_array(&r?, "message id")).collect()
    }

    /// A message by its id. Returns it with its `deleted` flag (deleted messages are tombstones
    /// with an empty body).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error or [`StorageError::InvalidData`].
    pub fn message_by_msg_id(&self, msg_id: &[u8; 16]) -> Result<Option<(StoredMessage, bool)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, conversation_id, msg_id, sender_device, outgoing, sent_at, received_at,
                    status, body, file_id, edited_at, deleted
             FROM messages WHERE msg_id = ?1",
        )?;
        stmt.query_row([&msg_id[..]], |r| {
            let deleted: bool = r.get(11)?;
            Ok(message_from_row(r)?.map(|m| (m, deleted)))
        })
        .optional()?
        .transpose()
    }

    /// Replaces the text of a message. Returns `false` if it does not exist or was deleted.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn edit_message(&self, msg_id: &[u8; 16], body: &str, edited_at: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE messages SET body = ?2, edited_at = ?3 WHERE msg_id = ?1 AND deleted = 0",
            params![&msg_id[..], body, edited_at],
        )? == 1)
    }

    /// Deletes a message, keeping a tombstone (empty body) so a late duplicate is ignored.
    /// Returns `false` if it does not exist or was already deleted.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn delete_message(&self, msg_id: &[u8; 16]) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE messages SET deleted = 1, body = '', edited_at = NULL
             WHERE msg_id = ?1 AND deleted = 0",
            [&msg_id[..]],
        )? == 1)
    }

    /// Number of unread incoming messages from a contact.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn unread_count(&self, contact: ContactId) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages m JOIN conversations c ON c.id = m.conversation_id
             WHERE c.contact_id = ?1 AND m.outgoing = 0 AND m.status = 4 AND m.deleted = 0",
            [contact.0],
            |r| r.get(0),
        )?)
    }

    /// Updates the status of a message. Statuses only move forward (never back from
    /// `Read` to `Delivered`, for instance).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn advance_message_status(&self, msg_id: &[u8; 16], status: MessageStatus) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET status = ?1 WHERE msg_id = ?2 AND outgoing = 1 AND status < ?1",
            params![status as i64, &msg_id[..]],
        )?;
        Ok(())
    }

    // --- secrets (crypto state) -----------------------------------------------------------

    /// Stores a named secret blob (e.g. serialized crypto session).
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn put_secret(&self, name: &str, value: &[u8], now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO secrets (name, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![name, value, now],
        )?;
        Ok(())
    }

    /// Loads a named secret blob.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn get_secret(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM secrets WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?
            .map(Zeroizing::new))
    }

    /// Deletes a named secret blob.
    ///
    /// # Errors
    ///
    /// Returns an SQLite error.
    pub fn delete_secret(&self, name: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM secrets WHERE name = ?1", [name])?;
        Ok(())
    }
}

fn to_array<const N: usize>(v: &[u8], what: &'static str) -> Result<[u8; N]> {
    <[u8; N]>::try_from(v).map_err(|_| StorageError::InvalidData(what))
}

fn contact_from_row(r: &Row<'_>) -> rusqlite::Result<Result<Contact>> {
    let master: Vec<u8> = r.get(1)?;
    Ok((|| {
        Ok(Contact {
            id: ContactId(r.get(0)?),
            master_pub: to_array(&master, "contact key")?,
            handle: r.get(2)?,
            display_name: r.get(3)?,
            verified: r.get(4)?,
            added_at: r.get(5)?,
        })
    })())
}

fn message_from_row(r: &Row<'_>) -> rusqlite::Result<Result<StoredMessage>> {
    let msg_id: Vec<u8> = r.get(2)?;
    let sender: Vec<u8> = r.get(3)?;
    let status: i64 = r.get(7)?;
    let file_id: Option<Vec<u8>> = r.get(9)?;
    Ok((|| {
        Ok(StoredMessage {
            id: r.get(0)?,
            conversation: ConversationId(r.get(1)?),
            msg_id: to_array(&msg_id, "message id")?,
            sender_device: to_array(&sender, "sender key")?,
            outgoing: r.get(4)?,
            sent_at: r.get(5)?,
            received_at: r.get(6)?,
            status: MessageStatus::from_i64(status).ok_or(StorageError::InvalidData("status"))?,
            body: r.get(8)?,
            file_id: file_id.map(|f| to_array(&f, "file id")).transpose()?,
            edited_at: r.get(10)?,
        })
    })())
}

fn file_from_row(r: &Row<'_>) -> rusqlite::Result<Result<FileRow>> {
    let id: Vec<u8> = r.get(0)?;
    let key: Vec<u8> = r.get(5)?;
    let source: Vec<u8> = r.get(6)?;
    let state: i64 = r.get(10)?;
    let size: i64 = r.get(4)?;
    Ok((|| {
        Ok(FileRow {
            file_id: to_array(&id, "file id")?,
            conversation: ConversationId(r.get(1)?),
            outgoing: r.get(2)?,
            name: r.get(3)?,
            size: u64::try_from(size).map_err(|_| StorageError::InvalidData("size"))?,
            key: to_array(&key, "file key")?,
            source_device: to_array(&source, "source device")?,
            manifest: r.get(7)?,
            path: r.get(8)?,
            bitmap: r.get(9)?,
            state: FileState::from_i64(state).ok_or(StorageError::InvalidData("file state"))?,
            created_at: r.get(11)?,
        })
    })())
}

const FILE_COLUMNS: &str =
    "file_id, conversation_id, outgoing, name, size, file_key, source_device,
                            manifest, path, bitmap, state, created_at";

#[cfg(test)]
mod tests {
    use super::Storage;
    use crate::{DbKey, LocalIdentity, MessageStatus, NewContact, NewMessage, StorageError};

    fn mem() -> Storage {
        Storage::open_in_memory(&DbKey::generate().unwrap()).unwrap()
    }

    #[test]
    fn file_is_encrypted_and_key_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ow.db");
        let key = DbKey::generate().unwrap();
        {
            let s = Storage::open(&path, &key).unwrap();
            assert!(!s.cipher_version().unwrap().is_empty());
            s.put_secret("probe", b"top secret value", 1).unwrap();
            // Flush WAL into the main file.
            s.conn
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .unwrap();
        }
        let raw = std::fs::read(&path).unwrap();
        assert!(
            !raw.starts_with(b"SQLite format 3"),
            "header must be encrypted"
        );
        assert!(
            !raw.windows(16).any(|w| w == b"top secret value"),
            "plaintext leaked"
        );

        let wrong = DbKey::generate().unwrap();
        assert!(matches!(
            Storage::open(&path, &wrong),
            Err(StorageError::WrongKeyOrCorrupted)
        ));

        let s = Storage::open(&path, &key).unwrap();
        assert_eq!(
            &**s.get_secret("probe").unwrap().unwrap(),
            b"top secret value"
        );
    }

    #[test]
    fn migrations_are_applied_once() {
        let s = mem();
        assert_eq!(s.schema_version().unwrap(), crate::migrations::latest());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), crate::migrations::latest());
    }

    #[test]
    fn refuses_newer_schema() {
        let s = mem();
        s.conn.pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(
            s.migrate(),
            Err(StorageError::TooNew { found: 99, .. })
        ));
    }

    #[test]
    fn local_identity_roundtrip() {
        let s = mem();
        assert!(s.load_local_identity().unwrap().is_none());
        let id = LocalIdentity {
            handle_name: "jean".into(),
            device_secret: [7; 32],
            device_cert: vec![1, 2, 3],
            master_secret: Some([9; 32]),
            created_at: 42,
        };
        s.save_local_identity(&id).unwrap();
        assert_eq!(s.load_local_identity().unwrap().unwrap(), id);
    }

    #[test]
    fn contacts_conversations_messages() {
        let s = mem();
        let c = s
            .upsert_contact(&NewContact {
                master_pub: [1; 32],
                handle: "alice#abcd".into(),
                display_name: None,
                added_at: 1,
            })
            .unwrap();
        // Upsert on the same key keeps the id.
        let c2 = s
            .upsert_contact(&NewContact {
                master_pub: [1; 32],
                handle: "alice2#abcd".into(),
                display_name: None,
                added_at: 2,
            })
            .unwrap();
        assert_eq!(c, c2);
        assert_eq!(s.contacts().unwrap()[0].handle, "alice2#abcd");
        s.add_contact_device(c, &[2; 32], b"cert").unwrap();
        assert_eq!(s.contact_devices(c).unwrap(), vec![[2; 32]]);
        s.set_contact_verified(c, true).unwrap();
        assert!(s.contact_by_master(&[1; 32]).unwrap().unwrap().verified);

        let conv = s.dm_conversation(c, 3).unwrap();
        assert_eq!(conv, s.dm_conversation(c, 4).unwrap());

        let mut m = NewMessage {
            conversation: conv,
            msg_id: [0; 16],
            sender_device: [2; 32],
            outgoing: true,
            sent_at: 0,
            received_at: None,
            status: MessageStatus::Pending,
            body: String::new(),
            file_id: None,
        };
        for i in 0..5u8 {
            m.msg_id = [i; 16];
            m.sent_at = i64::from(i);
            m.body = format!("msg {i}");
            assert!(s.insert_message(&m).unwrap().is_some());
        }
        // Duplicate delivery is ignored.
        assert!(s.insert_message(&m).unwrap().is_none());

        let page1 = s.messages(conv, None, 3).unwrap();
        assert_eq!(
            page1.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
            ["msg 4", "msg 3", "msg 2"]
        );
        let page2 = s.messages(conv, Some(page1[2].id), 3).unwrap();
        assert_eq!(page2.len(), 2);

        s.advance_message_status(&[4; 16], MessageStatus::Read)
            .unwrap();
        s.advance_message_status(&[4; 16], MessageStatus::Delivered)
            .unwrap();
        assert_eq!(
            s.messages(conv, None, 1).unwrap()[0].status,
            MessageStatus::Read
        );

        assert_eq!(s.contact_by_device(&[2; 32]).unwrap().unwrap().id, c);
        assert_eq!(s.contact(c).unwrap().unwrap().master_pub, [1; 32]);
        assert_eq!(
            s.undelivered(conv).unwrap().len(),
            4,
            "msg 4 is read, others pending"
        );

        let incoming = NewMessage {
            msg_id: [9; 16],
            outgoing: false,
            status: MessageStatus::Received,
            ..m.clone()
        };
        s.insert_message(&incoming).unwrap();
        assert_eq!(s.mark_incoming_read(conv).unwrap(), vec![[9; 16]]);

        assert!(s.mark_incoming_read(conv).unwrap().is_empty(), "only once");

        // Files.
        let f = crate::FileRow {
            file_id: [7; 32],
            conversation: conv,
            outgoing: false,
            name: "clip.mp4".into(),
            size: 5_000_000_000,
            key: [8; 32],
            source_device: [2; 32],
            manifest: None,
            path: None,
            bitmap: None,
            state: crate::FileState::Offered,
            created_at: 5,
        };
        s.insert_file(&f).unwrap();
        s.insert_file(&f).unwrap();
        s.insert_message(&NewMessage {
            msg_id: [10; 16],
            file_id: Some([7; 32]),
            body: "clip.mp4".into(),
            ..incoming.clone()
        })
        .unwrap();
        assert_eq!(s.messages(conv, None, 1).unwrap()[0].file_id, Some([7; 32]));
        s.update_file(
            &[7; 32],
            Some(crate::FileState::Downloading),
            Some("/tmp/clip.mp4"),
            Some(b"m"),
            Some(&[1]),
        )
        .unwrap();
        s.update_file(&[7; 32], None, None, None, Some(&[3]))
            .unwrap();
        let got = s.file(&[7; 32]).unwrap().unwrap();
        assert_eq!(got.size, 5_000_000_000);
        assert_eq!(got.path.as_deref(), Some("/tmp/clip.mp4"));
        assert_eq!(got.bitmap.as_deref(), Some(&[3][..]));
        assert_eq!(
            s.files_in_state(&[crate::FileState::Downloading])
                .unwrap()
                .len(),
            1
        );
        assert!(
            s.files_in_state(&[crate::FileState::Paused])
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.conversation_contact(conv).unwrap(), Some(c));

        // Edit and delete.
        assert!(s.edit_message(&[10; 16], "renamed", 99).unwrap());
        let (em, deleted) = s.message_by_msg_id(&[10; 16]).unwrap().unwrap();
        assert_eq!(
            (em.body.as_str(), em.edited_at, deleted),
            ("renamed", Some(99), false)
        );
        let before = s.messages(conv, None, 100).unwrap().len();
        assert!(s.delete_message(&[10; 16]).unwrap());
        assert!(!s.delete_message(&[10; 16]).unwrap());
        assert!(
            !s.edit_message(&[10; 16], "back", 100).unwrap(),
            "no edit after delete"
        );
        assert_eq!(s.messages(conv, None, 100).unwrap().len(), before - 1);
        assert!(s.message_by_msg_id(&[10; 16]).unwrap().unwrap().1);
        // A late duplicate does not bring it back.
        assert!(
            s.insert_message(&NewMessage {
                msg_id: [10; 16],
                ..incoming.clone()
            })
            .unwrap()
            .is_none()
        );
        assert_eq!(s.messages(conv, None, 100).unwrap().len(), before - 1);

        // Key change: a new contact entry (same person, new identity) is merged into the old one.
        let c_new = s
            .upsert_contact(&NewContact {
                master_pub: [5; 32],
                handle: "alice2".into(),
                display_name: None,
                added_at: 3,
            })
            .unwrap();
        s.add_contact_device(c_new, &[6; 32], b"cert2").unwrap();
        let conv_new = s.dm_conversation(c_new, 3).unwrap();
        let mut m2 = m.clone();
        m2.conversation = conv_new;
        m2.msg_id = [42; 16];
        s.insert_message(&m2).unwrap();
        let before = s.contact_messages(c, 100).unwrap().len();
        s.merge_contact(c_new, c, &[5; 32]).unwrap();
        let merged = s.contact(c).unwrap().unwrap();
        assert_eq!(merged.master_pub, [5; 32]);
        assert!(!merged.verified);
        assert_eq!(s.contact_devices(c).unwrap(), vec![[6; 32]]);
        assert_eq!(s.contact_messages(c, 100).unwrap().len(), before + 1);
        assert!(s.contact(c_new).unwrap().is_none());

        s.delete_contact(c).unwrap();
        assert!(
            s.messages(conv, None, 10).unwrap().is_empty(),
            "cascade delete"
        );
    }
}
