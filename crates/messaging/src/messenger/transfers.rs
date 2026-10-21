//! File transfers: offers, downloads (resumable, multi-source) and serving our files.
//!
//! The offer travels as an end-to-end encrypted message carrying the file id, size and key; the
//! data moves over the `/outpostwave/file/1` protocol (see `ow-files`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ow_files::{
    Bitmap, Download, FileKey, Manifest, Outcome, PartFile, Request, Response, Shared, Status,
    Transport,
};
use ow_p2p::{PeerId, ReplyToken, device_key_of, peer_id_of};
use ow_storage::{ContactId, FileRow, FileState, MessageStatus, NewMessage, StoredMessage};
use tokio::sync::watch;

use super::{Inner, Messenger, MessengerEvent, Result, lock, now_ms};
use crate::MessagingError;

/// Incoming files up to this size are downloaded automatically.
pub const AUTO_DOWNLOAD_MAX: u64 = 25 * 1024 * 1024;
/// Parallel file requests we serve.
const SERVE_SLOTS: usize = 64;
/// Progress events at most this often per file.
const EVENT_EVERY: Duration = Duration::from_millis(250);

/// A file transfer, as shown to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct FileInfo {
    /// File id.
    pub file_id: [u8; 32],
    /// Contact of the conversation.
    pub contact: ContactId,
    /// File name.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// `true` if we sent it.
    pub outgoing: bool,
    /// State.
    pub state: FileState,
    /// Bytes received (incoming) or sent to the contact (outgoing, this session).
    pub bytes: u64,
    /// Current throughput, bytes per second.
    pub rate: f64,
    /// Local path: final file (complete download) or original (outgoing).
    pub path: Option<PathBuf>,
}

#[derive(Debug)]
struct Active {
    cancel: watch::Sender<bool>,
    /// The user cancelled (not paused): drop the partial file.
    abort: bool,
    bytes: u64,
    rate: f64,
}

#[derive(Debug, Default)]
struct Served {
    chunks: HashSet<u32>,
    bytes: u64,
    started: Option<Instant>,
    last_event: Option<Instant>,
}

/// Transfer state kept in memory.
#[derive(Debug)]
pub(super) struct Transfers {
    active: HashMap<[u8; 32], Active>,
    served: HashMap<[u8; 32], Served>,
    shared: HashMap<[u8; 32], Arc<Manifest>>,
    download_dir: Option<PathBuf>,
    upload_limit: Option<u64>,
    upload_next: Option<Instant>,
    serve_slots: Arc<tokio::sync::Semaphore>,
}

impl Default for Transfers {
    fn default() -> Self {
        Self {
            active: HashMap::new(),
            served: HashMap::new(),
            shared: HashMap::new(),
            download_dir: None,
            upload_limit: None,
            upload_next: None,
            serve_slots: Arc::new(tokio::sync::Semaphore::new(SERVE_SLOTS)),
        }
    }
}

/// File protocol over the P2P node.
struct P2pTransport(Arc<Inner>);

impl Transport for P2pTransport {
    type Source = PeerId;

    async fn request(
        &self,
        source: &PeerId,
        request: Vec<u8>,
    ) -> std::result::Result<Vec<u8>, String> {
        let p2p = &self.0.p2p;
        p2p.dial(*source, self.0.dial_addrs(*source))
            .await
            .map_err(|e| e.to_string())?;
        p2p.file_request(*source, request)
            .await
            .map_err(|e| e.to_string())
    }
}

fn chunk_bytes(bitmap: Option<&[u8]>, manifest: Option<&Manifest>, size: u64) -> u64 {
    match (bitmap, manifest) {
        (Some(b), Some(m)) => Bitmap::from_bytes(b, m.count()).map_or(0, |b| {
            (0..b.len())
                .filter(|&i| b.get(i))
                .map(|i| m.chunk_len(i) as u64)
                .sum()
        }),
        _ => {
            let _ = size;
            0
        }
    }
}

impl Messenger {
    /// Folder where incoming files are saved (automatic downloads and default destination).
    pub fn set_download_dir(&self, dir: PathBuf) {
        lock(&self.inner.transfers).download_dir = Some(dir);
    }

    /// Limits the upload throughput of file transfers (`None`: unlimited).
    pub fn set_upload_limit(&self, bytes_per_second: Option<u64>) {
        lock(&self.inner.transfers).upload_limit = bytes_per_second.filter(|b| *b > 0);
    }

    /// Offers a file to a contact. The file is read once to build its manifest (about 1 s per
    /// GB), then stays where it is: it is served from there while the contact downloads it.
    ///
    /// # Errors
    ///
    /// I/O errors, [`MessagingError::UnknownContact`] or storage errors.
    pub async fn send_file(&self, contact: ContactId, path: &Path) -> Result<StoredMessage> {
        let inner = &self.inner;
        // `absolute`, not `canonicalize`: no `\\?\` prefix on Windows (explorer cannot show it).
        let path = std::path::absolute(path).map_err(|e| MessagingError::Io(e.to_string()))?;
        if !path.is_file() {
            return Err(MessagingError::Io("not a file".into()));
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(ow_files::safe_file_name)
            .ok_or_else(|| MessagingError::Io("invalid file name".into()))?;
        let key = FileKey::generate().map_err(|_| MessagingError::Io("RNG failure".into()))?;
        let device = ow_identity::DeviceKey::from_secret_bytes(&inner.device.to_secret_bytes());
        let (p, k) = (path.clone(), key.clone());
        let manifest =
            tokio::task::spawn_blocking(move || ow_files::prepare(&p, &k, &device, |_| {}))
                .await
                .map_err(|_| MessagingError::Io("interrupted".into()))?
                .map_err(|e| MessagingError::Io(e.to_string()))?;
        let now = now_ms();
        let s = lock(&inner.storage);
        if s.contact(contact)?.is_none() {
            return Err(MessagingError::UnknownContact);
        }
        let conv = s.dm_conversation(contact, now)?;
        s.insert_file(&FileRow {
            file_id: *manifest.id(),
            conversation: conv,
            outgoing: true,
            name: name.clone(),
            size: manifest.size(),
            key: *key.as_bytes(),
            source_device: inner.device.public_key().to_bytes(),
            manifest: Some(manifest.as_bytes().to_vec()),
            path: path.to_str().map(str::to_owned),
            bitmap: None,
            state: FileState::Complete,
            created_at: now,
        })?;
        let mut msg_id = [0u8; 16];
        getrandom::fill(&mut msg_id).map_err(|_| MessagingError::Io("RNG failure".into()))?;
        let new = NewMessage {
            conversation: conv,
            msg_id,
            sender_device: inner.device.public_key().to_bytes(),
            outgoing: true,
            sent_at: now,
            received_at: None,
            status: MessageStatus::Pending,
            body: name,
            file_id: Some(*manifest.id()),
        };
        let id = s
            .insert_message(&new)?
            .ok_or(MessagingError::UnknownContact)?;
        drop(s);
        lock(&inner.transfers)
            .shared
            .insert(*manifest.id(), Arc::new(manifest));
        let message = StoredMessage {
            id,
            conversation: conv,
            msg_id,
            sender_device: new.sender_device,
            outgoing: true,
            sent_at: now,
            received_at: None,
            status: MessageStatus::Pending,
            body: new.body,
            file_id: new.file_id,
        };
        let _ = inner.events.send(MessengerEvent::Message {
            contact,
            message: message.clone(),
        });
        inner
            .backup_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Inner::spawn_delivery(inner, contact);
        Ok(message)
    }

    /// Starts or resumes downloading an incoming file into `dir` (default: the download folder).
    ///
    /// # Errors
    ///
    /// [`MessagingError::UnknownFile`], [`MessagingError::Io`] if no folder is set, storage
    /// errors.
    pub fn download_file(&self, file_id: &[u8; 32], dir: Option<PathBuf>) -> Result<()> {
        let inner = &self.inner;
        let row = lock(&inner.storage)
            .file(file_id)?
            .ok_or(MessagingError::UnknownFile)?;
        if row.outgoing || row.state == FileState::Complete {
            return Ok(());
        }
        if row.path.is_none() {
            let dir = dir
                .or_else(|| lock(&inner.transfers).download_dir.clone())
                .ok_or_else(|| MessagingError::Io("no download folder".into()))?;
            let target = ow_files::unique_path(&dir.join(&row.name));
            lock(&inner.storage).update_file(file_id, None, target.to_str(), None, None)?;
        }
        if row.state == FileState::Failed {
            // Restart from scratch.
            lock(&inner.storage).update_file(
                file_id,
                Some(FileState::Offered),
                None,
                None,
                Some(&[]),
            )?;
        }
        Inner::spawn_download(inner, *file_id);
        Ok(())
    }

    /// Pauses a download (resumable).
    ///
    /// # Errors
    ///
    /// Storage errors.
    pub fn pause_file(&self, file_id: &[u8; 32]) -> Result<()> {
        let inner = &self.inner;
        if let Some(a) = lock(&inner.transfers).active.get(file_id) {
            let _ = a.cancel.send(true);
            return Ok(());
        }
        lock(&inner.storage).update_file(file_id, Some(FileState::Paused), None, None, None)?;
        inner.emit_file(file_id);
        Ok(())
    }

    /// Cancels a transfer: an incoming download stops and its partial file is deleted; an
    /// outgoing file is no longer served.
    ///
    /// # Errors
    ///
    /// [`MessagingError::UnknownFile`] or storage errors.
    pub fn cancel_file(&self, file_id: &[u8; 32]) -> Result<()> {
        let inner = &self.inner;
        let row = lock(&inner.storage)
            .file(file_id)?
            .ok_or(MessagingError::UnknownFile)?;
        {
            let mut t = lock(&inner.transfers);
            t.shared.remove(file_id);
            if let Some(a) = t.active.get_mut(file_id) {
                a.abort = true;
                let _ = a.cancel.send(true);
                return Ok(());
            }
        }
        if !row.outgoing && row.state != FileState::Complete {
            if let Some(p) = &row.path {
                let _ = std::fs::remove_file(PartFile::part_path(Path::new(p)));
            }
        }
        lock(&inner.storage).update_file(file_id, Some(FileState::Failed), None, None, None)?;
        inner.emit_file(file_id);
        Ok(())
    }

    /// Details of a file transfer.
    ///
    /// # Errors
    ///
    /// [`MessagingError::UnknownFile`] or storage errors.
    pub fn file(&self, file_id: &[u8; 32]) -> Result<FileInfo> {
        self.inner.file_info(file_id)
    }
}

impl Inner {
    pub(super) fn file_info(&self, file_id: &[u8; 32]) -> Result<FileInfo> {
        let (row, contact) = {
            let s = lock(&self.storage);
            let row = s.file(file_id)?.ok_or(MessagingError::UnknownFile)?;
            let contact = s
                .conversation_contact(row.conversation)?
                .ok_or(MessagingError::UnknownFile)?;
            (row, contact)
        };
        let t = lock(&self.transfers);
        let (bytes, rate) = if row.outgoing {
            t.served.get(file_id).map_or((0, 0.0), |s| {
                let secs = s
                    .started
                    .map_or(1.0, |st| st.elapsed().as_secs_f64().max(0.5));
                #[allow(clippy::cast_precision_loss)]
                (s.bytes.min(row.size), s.bytes as f64 / secs)
            })
        } else if row.state == FileState::Complete {
            (row.size, 0.0)
        } else if let Some(a) = t.active.get(file_id).filter(|a| a.bytes > 0) {
            (a.bytes, a.rate)
        } else {
            let m = row
                .manifest
                .as_deref()
                .and_then(|m| Manifest::parse(m, file_id).ok());
            (
                chunk_bytes(row.bitmap.as_deref(), m.as_ref(), row.size),
                0.0,
            )
        };
        let path = match (row.outgoing, row.state) {
            (true, _) | (false, FileState::Complete) => row.path.as_ref().map(PathBuf::from),
            _ => None,
        };
        Ok(FileInfo {
            file_id: row.file_id,
            contact,
            name: row.name,
            size: row.size,
            outgoing: row.outgoing,
            state: row.state,
            bytes,
            rate,
            path,
        })
    }

    pub(super) fn emit_file(&self, file_id: &[u8; 32]) {
        if let Ok(file) = self.file_info(file_id) {
            let _ = self.events.send(MessengerEvent::File {
                contact: file.contact,
                file,
            });
        }
    }

    /// Records an incoming offer. Small files start downloading at once.
    pub(super) fn on_offer(
        self: &Arc<Self>,
        conversation: ow_storage::ConversationId,
        device: [u8; 32],
        file: &crate::wire::FileRef,
        name: &str,
    ) -> Result<()> {
        let row = FileRow {
            file_id: file.file_id,
            conversation,
            outgoing: false,
            name: name.to_owned(),
            size: file.size,
            key: file.key,
            source_device: device,
            manifest: None,
            path: None,
            bitmap: None,
            state: FileState::Offered,
            created_at: now_ms(),
        };
        lock(&self.storage).insert_file(&row)?;
        let dir = lock(&self.transfers).download_dir.clone();
        if file.size <= AUTO_DOWNLOAD_MAX {
            if let Some(dir) = dir {
                let target = ow_files::unique_path(&dir.join(name));
                lock(&self.storage).update_file(
                    &file.file_id,
                    None,
                    target.to_str(),
                    None,
                    None,
                )?;
                Self::spawn_download(self, file.file_id);
            }
        }
        Ok(())
    }

    /// Resumes downloads interrupted by a restart, or waiting for `contact` to come back.
    pub(super) fn resume_downloads(self: &Arc<Self>, contact: Option<ContactId>) {
        let states: &[FileState] = if contact.is_some() {
            &[FileState::Waiting]
        } else {
            &[FileState::Downloading, FileState::Waiting]
        };
        let rows = lock(&self.storage)
            .files_in_state(states)
            .unwrap_or_default();
        for row in rows {
            if row.outgoing {
                continue;
            }
            if let Some(c) = contact {
                let of = lock(&self.storage)
                    .conversation_contact(row.conversation)
                    .ok()
                    .flatten();
                if of != Some(c) {
                    continue;
                }
            }
            Self::spawn_download(self, row.file_id);
        }
    }

    pub(super) fn spawn_download(self: &Arc<Self>, file_id: [u8; 32]) {
        let (tx, rx) = watch::channel(false);
        {
            let mut t = lock(&self.transfers);
            if t.active.contains_key(&file_id) {
                return;
            }
            t.active.insert(
                file_id,
                Active {
                    cancel: tx,
                    abort: false,
                    bytes: 0,
                    rate: 0.0,
                },
            );
        }
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            let result = inner.run_download(file_id, rx).await;
            let abort = lock(&inner.transfers)
                .active
                .remove(&file_id)
                .is_some_and(|a| a.abort);
            let state = match result {
                Ok(_) if abort => {
                    if let Ok(Some(row)) = lock(&inner.storage).file(&file_id) {
                        if let Some(p) = &row.path {
                            let _ = std::fs::remove_file(PartFile::part_path(Path::new(p)));
                        }
                    }
                    FileState::Failed
                }
                Ok(state) => state,
                Err(e) => {
                    tracing::warn!(%e, "download failed");
                    FileState::Failed
                }
            };
            let _ = lock(&inner.storage).update_file(&file_id, Some(state), None, None, None);
            inner.emit_file(&file_id);
        });
    }

    async fn run_download(
        self: &Arc<Self>,
        file_id: [u8; 32],
        cancel: watch::Receiver<bool>,
    ) -> Result<FileState> {
        let row = lock(&self.storage)
            .file(&file_id)?
            .ok_or(MessagingError::UnknownFile)?;
        let target = PathBuf::from(
            row.path
                .clone()
                .ok_or_else(|| MessagingError::Io("no destination".into()))?,
        );
        let contact = lock(&self.storage)
            .conversation_contact(row.conversation)?
            .ok_or(MessagingError::UnknownFile)?;
        lock(&self.storage).update_file(
            &file_id,
            Some(FileState::Downloading),
            None,
            None,
            None,
        )?;
        self.emit_file(&file_id);

        // Sources: the device that offered the file first, then the contact's other devices.
        let mut sources = vec![peer_id_of(&row.source_device)?];
        for d in lock(&self.storage).contact_devices(contact)? {
            let p = peer_id_of(&d)?;
            if !sources.contains(&p) {
                sources.push(p);
            }
        }
        let transport = P2pTransport(Arc::clone(self));
        let manifest = match row
            .manifest
            .as_deref()
            .map(|m| Manifest::parse(m, &file_id))
        {
            Some(Ok(m)) => m,
            _ => match ow_files::fetch_manifest(&transport, &sources, &file_id).await {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(%e, "manifest unavailable, waiting for the sender");
                    return Ok(FileState::Waiting);
                }
            },
        };
        if manifest.size() != row.size {
            return Err(MessagingError::Io("size does not match the offer".into()));
        }
        lock(&self.storage).update_file(&file_id, None, None, Some(manifest.as_bytes()), None)?;
        let part = PartFile::open(&target, manifest.size())
            .map_err(|e| MessagingError::Io(e.to_string()))?;
        let done = row
            .bitmap
            .as_deref()
            .and_then(|b| Bitmap::from_bytes(b, manifest.count()))
            .unwrap_or_else(|| Bitmap::new(manifest.count()));
        let key = FileKey::from_bytes(row.key);
        let mut last_event = Instant::now() - EVENT_EVERY;
        let outcome = ow_files::download(
            &transport,
            Download {
                manifest: &manifest,
                key: &key,
                part: &part,
                done,
                sources,
                new_sources: None,
                cancel,
            },
            |p, persist| {
                if let Some(a) = lock(&self.transfers).active.get_mut(&file_id) {
                    a.bytes = p.bytes;
                    a.rate = p.rate;
                }
                if let Some(b) = persist {
                    let _ = lock(&self.storage).update_file(
                        &file_id,
                        None,
                        None,
                        None,
                        Some(b.as_bytes()),
                    );
                }
                if last_event.elapsed() >= EVENT_EVERY {
                    last_event = Instant::now();
                    self.emit_file(&file_id);
                }
            },
        )
        .await;
        Ok(match outcome {
            Outcome::Complete => {
                let fin = part
                    .finish()
                    .map_err(|e| MessagingError::Io(e.to_string()))?;
                lock(&self.storage).update_file(
                    &file_id,
                    Some(FileState::Complete),
                    fin.to_str(),
                    None,
                    Some(Bitmap::full(manifest.count()).as_bytes()),
                )?;
                FileState::Complete
            }
            Outcome::Stalled(b) => {
                lock(&self.storage).update_file(&file_id, None, None, None, Some(b.as_bytes()))?;
                FileState::Waiting
            }
            Outcome::Cancelled(b) => {
                lock(&self.storage).update_file(&file_id, None, None, None, Some(b.as_bytes()))?;
                FileState::Paused
            }
            Outcome::Changed(_) => FileState::Failed,
            Outcome::Failed(b, e) => {
                tracing::warn!(%e, "download stopped (disk)");
                lock(&self.storage).update_file(&file_id, None, None, None, Some(b.as_bytes()))?;
                FileState::Paused
            }
        })
    }

    /// What we can serve for `file_id` to `requester`, if it may access the file.
    fn shared_for(&self, file_id: &[u8; 32], requester: &[u8; 32]) -> Option<Shared> {
        let s = lock(&self.storage);
        let row = s.file(file_id).ok()??;
        // Only the devices of the conversation's contact (and our own devices) may fetch it.
        let contact = s.conversation_contact(row.conversation).ok()??;
        let allowed = s.contact_devices(contact).ok()?.contains(requester)
            || requester == &self.device.public_key().to_bytes();
        if !allowed {
            return None;
        }
        drop(s);
        let manifest = {
            let cached = lock(&self.transfers).shared.get(file_id).cloned();
            match cached {
                Some(m) => m,
                None => {
                    let m = Arc::new(Manifest::parse(row.manifest.as_deref()?, file_id).ok()?);
                    lock(&self.transfers)
                        .shared
                        .insert(*file_id, Arc::clone(&m));
                    m
                }
            }
        };
        let path = PathBuf::from(row.path?);
        let (path, have) = match (row.outgoing, row.state) {
            (true, FileState::Complete) | (false, FileState::Complete) => (path, None),
            (true, _) => return None,
            (false, FileState::Failed) => return None,
            (false, _) => (
                PartFile::part_path(&path),
                Some(Bitmap::from_bytes(
                    row.bitmap.as_deref()?,
                    manifest.count(),
                )?),
            ),
        };
        Some(Shared {
            manifest: (*manifest).clone(),
            key: FileKey::from_bytes(row.key),
            path,
            have,
        })
    }

    /// Answers a file-protocol request.
    pub(super) fn serve(self: &Arc<Self>, from: PeerId, payload: Vec<u8>, token: ReplyToken) {
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            let slots = Arc::clone(&lock(&inner.transfers).serve_slots);
            let response = match slots.try_acquire() {
                Err(_) => Response::error(Status::Busy),
                Ok(_permit) => inner.serve_one(from, &payload).await,
            };
            let _ = inner.p2p.file_reply(token, response.encode()).await;
        });
    }

    async fn serve_one(self: &Arc<Self>, from: PeerId, payload: &[u8]) -> Response {
        let Some(request) = Request::decode(payload) else {
            return Response::error(Status::Unknown);
        };
        let Some(requester) = device_key_of(&from) else {
            return Response::error(Status::Unknown);
        };
        let Some(shared) = self.shared_for(request.file_id(), &requester) else {
            return Response::error(Status::Unknown);
        };
        let outgoing = shared.have.is_none();
        let total = shared.manifest.count();
        let response = tokio::task::spawn_blocking(move || ow_files::respond(&shared, &request))
            .await
            .unwrap_or_else(|_| Response::error(Status::Busy));
        if let (Request::Chunk(id, i), Status::Ok) = (request, response.status) {
            // Optional upload limit: space chunks out.
            let wait = {
                let mut t = lock(&self.transfers);
                match t.upload_limit {
                    Some(limit) => {
                        let now = Instant::now();
                        let start = t.upload_next.filter(|n| *n > now).unwrap_or(now);
                        #[allow(clippy::cast_precision_loss)]
                        let cost =
                            Duration::from_secs_f64(response.body.len() as f64 / limit as f64);
                        t.upload_next = Some(start + cost);
                        start.saturating_duration_since(now)
                    }
                    None => Duration::ZERO,
                }
            };
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
            if outgoing {
                self.track_upload(&id, i, response.body.len(), total);
            }
        }
        response
    }

    fn track_upload(&self, file_id: &[u8; 32], index: u32, len: usize, total: u32) {
        let emit = {
            let mut t = lock(&self.transfers);
            let s = t.served.entry(*file_id).or_default();
            s.started.get_or_insert_with(Instant::now);
            if s.chunks.insert(index) {
                s.bytes += len.saturating_sub(16) as u64;
            }
            let last = s.chunks.len() == total as usize;
            let due = last || s.last_event.is_none_or(|l| l.elapsed() >= EVENT_EVERY);
            if due {
                s.last_event = Some(Instant::now());
            }
            due
        };
        if emit {
            self.emit_file(file_id);
        }
    }
}
