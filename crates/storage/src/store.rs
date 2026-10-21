//! Database access.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use zeroize::{Zeroize, Zeroizing};

use crate::migrations::{MIGRATIONS, latest};
use crate::{
    Contact, ContactId, ConversationId, DbKey, LocalIdentity, MessageStatus, NewContact,
    NewMessage, StorageError, StoredMessage,
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
                                       received_at, status, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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
                    m.body
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
                    status, body
             FROM messages
             WHERE conversation_id = ?1 AND id < ?2
             ORDER BY sent_at DESC, id DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![conversation.0, before.unwrap_or(i64::MAX), limit],
            message_from_row,
        )?;
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
                    status, body
             FROM messages
             WHERE conversation_id = ?1 AND outgoing = 1 AND status < 2
             ORDER BY sent_at, id",
        )?;
        let rows = stmt.query_map([conversation.0], message_from_row)?;
        rows.map(|r| r?).collect()
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
        })
    })())
}

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
        assert_eq!(s.schema_version().unwrap(), 1);
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), 1);
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

        s.delete_contact(c).unwrap();
        assert!(
            s.messages(conv, None, 10).unwrap().is_empty(),
            "cascade delete"
        );
    }
}
