// Main screen: contact list + conversation.
import { createEffect, createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { createStore, produce } from "solid-js/store";
import {
  api,
  errorText,
  type Contact,
  type FileTransfer,
  type Invite,
  type Message,
  type Settings,
  mediaUrl,
} from "../api";
import { Dialog } from "../components/Dialog";
import { chime, setBadge } from "../notify";
import { locale, t, type Key } from "../i18n";

const timeFmt = new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" });
const numFmt = new Intl.NumberFormat(locale, { maximumFractionDigits: 1 });

/** Human-readable size (decimal units, as file managers show them). */
function formatSize(bytes: number): string {
  const units = locale.startsWith("fr") ? ["o", "Ko", "Mo", "Go", "To"] : ["B", "kB", "MB", "GB", "TB"];
  let v = bytes;
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return `${numFmt.format(v)} ${units[i]}`;
}

export function Main(props: { handle: string; os: string; server: string | null }) {
  const [contacts, setContacts] = createSignal<Contact[]>([]);
  const [sounds, setSounds] = createSignal(true);
  const loadPrefs = () => api.getPrefs().then((p) => setSounds(p.sounds)).catch(() => {});
  const [selected, setSelected] = createSignal<number | null>(null);
  // Files being read before they are offered, per contact.
  const [preparing, setPreparing] = createSignal<Record<number, { name: string; bytes: number; size: number }>>({});
  // Messages per contact, oldest first.
  const [messages, setMessages] = createStore<Record<number, Message[]>>({});
  const [dialog, setDialog] = createSignal<"invite" | "add" | "safety" | "settings" | null>(null);

  const current = () => contacts().find((c) => c.id === selected()) ?? null;

  async function loadContacts() {
    setContacts(await api.listContacts());
  }

  async function loadMessages(contact: number) {
    const list = await api.listMessages(contact);
    setMessages(contact, list.reverse());
  }

  async function loadOlder(contact: number) {
    const first = messages[contact]?.[0];
    if (!first) return;
    const older = await api.listMessages(contact, first.id);
    setMessages(contact, (cur) => [...older.reverse(), ...cur]);
  }

  const setUnread = (id: number, f: (n: number) => number) =>
    setContacts((list) => list.map((c) => (c.id === id ? { ...c, unread: f(c.unread) } : c)));

  // Read receipts: the open conversation is "read" while the window has focus.
  function markReadIfVisible() {
    const id = selected();
    if (id !== null && document.hasFocus()) {
      setUnread(id, () => 0);
      void api.markRead(id);
    }
  }

  // Taskbar badge: total unread messages (no badge at 0).
  createEffect(() => setBadge(contacts().reduce((n, c) => n + c.unread, 0), props.os));
  onCleanup(() => setBadge(0, props.os));

  function notify(kind: "message" | "activity") {
    if (sounds()) chime(kind);
    void api.requestAttention().catch(() => {});
  }

  onMount(async () => {
    window.addEventListener("focus", markReadIfVisible);
    onCleanup(() => window.removeEventListener("focus", markReadIfVisible));
    await loadContacts();
    void loadPrefs();
    const unlisten = await api.onEvent((e) => {
      switch (e.type) {
        case "message":
          if (messages[e.contact]) {
            setMessages(e.contact, produce((list) => {
              if (!list.some((m) => m.msgId === e.message.msgId)) list.push(e.message);
            }));
          }
          if (!e.message.outgoing) {
            if (e.contact === selected() && document.hasFocus()) {
              markReadIfVisible();
            } else {
              if (contacts().some((c) => c.id === e.contact)) setUnread(e.contact, (n) => n + 1);
              else void loadContacts();
              notify("message");
            }
          }
          break;
        case "preparing":
          setPreparing((all) => {
            const next = { ...all };
            if (e.bytes >= e.size) delete next[e.contact];
            else next[e.contact] = { name: e.name, bytes: e.bytes, size: e.size };
            return next;
          });
          break;
        case "messageEdited":
          if (messages[e.contact]) {
            setMessages(e.contact, (m) => m.msgId === e.message.msgId, e.message);
          }
          break;
        case "messageDeleted":
          if (messages[e.contact]) {
            setMessages(e.contact, (list) => list.filter((m) => m.msgId !== e.msgId));
          }
          break;
        case "file":
          if (messages[e.contact]) {
            setMessages(e.contact, (m) => m.file?.fileId === e.file.fileId, "file", e.file);
          }
          break;
        case "status":
          setMessages(e.contact, (m) => m.msgId === e.msgId, "status", e.status);
          break;
        case "presence":
          setContacts((list) => list.map((c) => (c.id === e.contact ? { ...c, online: e.online } : c)));
          break;
        case "contacts":
        case "contactRequest":
          void loadContacts();
          break;
        case "keyChanged":
          void loadContacts();
          notify("activity");
          break;
        case "resync":
          void loadContacts();
          if (selected() !== null) void loadMessages(selected()!);
          break;
      }
    });
    onCleanup(unlisten);
  });

  createEffect(() => {
    const id = selected();
    if (id !== null && !messages[id]) void loadMessages(id);
    markReadIfVisible();
  });

  return (
    <div class="layout">
      <aside class="sidebar">
        <div class="me">
          <strong>{props.handle}</strong>
          <button class="ghost" onClick={() => setDialog("settings")} title={t("settings")} aria-label={t("settings")}>
            ⚙
          </button>
        </div>
        <div class="actions">
          <button onClick={() => setDialog("invite")}>{t("myCode")}</button>
          <button class="primary" onClick={() => setDialog("add")}>
            {t("addContact")}
          </button>
        </div>
        <h3 class="muted">{t("contacts")}</h3>
        <Show when={contacts().length > 0} fallback={<p class="muted small">{t("noContacts")}</p>}>
          <ul class="contacts">
            <For each={contacts()}>
              {(c) => (
                <li>
                  <button classList={{ selected: c.id === selected() }} onClick={() => setSelected(c.id)}>
                    <span class="dot" classList={{ on: c.online }} aria-label={c.online ? t("online") : t("offline")} />
                    <span class="handle">{c.displayName ?? c.handle}</span>
                    <Show when={c.verified}>
                      <span class="badge" title={t("verified")}>✓</span>
                    </Show>
                    <Show when={c.keyChanged || c.nameUnconfirmed}>
                      <span class="alert" title={c.keyChanged ? t("keyChangedBanner").replace("{name}", c.handle) : t("nameUnconfirmedBanner")}>⚠</span>
                    </Show>
                    <Show when={c.unread > 0}>
                      <span class="unread" aria-label={`${c.unread}`}>{c.unread > 99 ? "99+" : c.unread}</span>
                    </Show>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </aside>

      <section class="chat">
        <Show when={current()} fallback={<div class="empty muted">{t("selectContact")}</div>}>
          {(c) => (
            <Conversation
              contact={c()}
              messages={messages[c().id] ?? []}
              onOlder={() => loadOlder(c().id)}
              onSafety={() => setDialog("safety")}
              preparing={preparing()[c().id] ?? null}
              onAck={async () => {
                await api.ackKeyChange(c().id);
                void loadContacts();
              }}
            />
          )}
        </Show>
      </section>

      <Show when={dialog() === "invite"}>
        <InviteDialog onClose={() => setDialog(null)} />
      </Show>
      <Show when={dialog() === "add"}>
        <AddDialog
          onClose={() => setDialog(null)}
          onAdded={(c) => {
            void loadContacts();
            setSelected(c.id);
            setDialog(null);
          }}
        />
      </Show>
      <Show when={dialog() === "settings"}>
        <SettingsDialog
          server={props.server}
          onClose={() => {
            setDialog(null);
            void loadPrefs();
          }}
        />
      </Show>
      <Show when={dialog() === "safety" && current()}>
        {(c) => (
          <SafetyDialog
            contact={c()}
            onClose={() => setDialog(null)}
            onChanged={() => void loadContacts()}
          />
        )}
      </Show>
    </div>
  );
}

function Conversation(props: {
  contact: Contact;
  messages: Message[];
  onOlder: () => void;
  onSafety: () => void;
  onAck: () => void;
  preparing: { name: string; bytes: number; size: number } | null;
}) {
  const [text, setText] = createSignal("");
  const [error, setError] = createSignal("");
  const [preparing, setPreparing] = createSignal(false);
  const [editing, setEditing] = createSignal<Message | null>(null);
  const [deleting, setDeleting] = createSignal<Message | null>(null);
  const [viewer, setViewer] = createSignal<FileTransfer | null>(null);
  let input: HTMLInputElement | undefined;

  function startEdit(m: Message) {
    setEditing(m);
    setText(m.body);
    queueMicrotask(() => input?.focus());
  }

  function stopEdit() {
    setEditing(null);
    setText("");
  }

  function onComposerKey(e: KeyboardEvent) {
    if (e.key === "Escape" && editing()) {
      e.preventDefault();
      stopEdit();
    } else if (e.key === "ArrowUp" && !text() && !editing()) {
      // Like Discord: edit the last message we wrote.
      const last = [...props.messages].reverse().find((m) => m.outgoing && !m.file);
      if (last) {
        e.preventDefault();
        startEdit(last);
      }
    }
  }
  const [dragging, setDragging] = createSignal(false);
  let list: HTMLDivElement | undefined;
  const name = () => props.contact.displayName ?? props.contact.handle;

  async function sendFiles(action: () => Promise<void>) {
    setError("");
    setPreparing(true);
    try {
      await action();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setPreparing(false);
    }
  }

  // Files dropped on the window (the native drop gives real paths; the browser one does not).
  onMount(async () => {
    try {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      const unlisten = await getCurrentWebview().onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "over" || p.type === "enter") setDragging(true);
        else if (p.type === "leave") setDragging(false);
        else if (p.type === "drop") {
          setDragging(false);
          if (p.paths.length > 0) void sendFiles(() => api.sendFiles(props.contact.id, p.paths));
        }
      });
      onCleanup(unlisten);
    } catch {
      // Not running in Tauri (UI preview).
    }
  });

  // Keep the view scrolled to the newest message.
  createEffect(() => {
    void props.messages.length;
    queueMicrotask(() => list && (list.scrollTop = list.scrollHeight));
  });

  async function send(e: Event) {
    e.preventDefault();
    const body = text().trim();
    if (!body) return;
    setError("");
    try {
      const target = editing();
      if (target) {
        if (body !== target.body) await api.editMessage(props.contact.id, target.msgId, body);
        stopEdit();
        return;
      }
      await api.sendMessage(props.contact.id, body);
      setText("");
    } catch (err) {
      setError(errorText(err));
    }
  }

  return (
    <>
      <header class="chat-header">
        <span class="dot" classList={{ on: props.contact.online }} />
        <strong>{props.contact.displayName ?? props.contact.handle}</strong>
        <span class="muted small">{props.contact.online ? t("online") : t("offline")}</span>
        <button class="ghost right" onClick={props.onSafety}>
          {t("safetyNumber")}
        </button>
      </header>
      <Show when={props.contact.keyChanged}>
        <div class="banner">
          <span>{t("keyChangedBanner").replace("{name}", props.contact.handle)}</span>
          <button onClick={props.onSafety}>{t("check")}</button>
          <button class="ghost" onClick={props.onAck}>{t("ok")}</button>
        </div>
      </Show>
      <Show when={props.contact.nameUnconfirmed}>
        <div class="banner">
          <span>{t("nameUnconfirmedBanner")}</span>
        </div>
      </Show>
      <div class="messages" ref={list}>
        <Show when={props.messages.length >= 50}>
          <button class="ghost center" onClick={props.onOlder}>
            {t("loadMore")}
          </button>
        </Show>
        <For each={props.messages}>
          {(m) => (
            <div class="bubble" classList={{ out: m.outgoing, file: !!m.file, editing: editing()?.msgId === m.msgId }}>
              <div class="msg-actions">
                <Show when={m.outgoing && !m.file}>
                  <button class="ghost" title={t("edit")} aria-label={t("edit")} onClick={() => startEdit(m)}>
                    ✏️
                  </button>
                </Show>
                <button class="ghost" title={t("delete")} aria-label={t("delete")} onClick={() => setDeleting(m)}>
                  🗑️
                </button>
              </div>
              <Show when={m.file} fallback={<p>{m.body}</p>}>
                {(f) => <FileCard file={f()} contactName={name()} onView={setViewer} />}
              </Show>
              <span class="meta">
                <Show when={m.editedAt}>{`${t("edited")} · `}</Show>
                {timeFmt.format(new Date(m.sentAt))}
                <Show when={m.outgoing}>{` · ${t(`status_${m.status}` as Key)}`}</Show>
              </span>
            </div>
          )}
        </For>
      </div>
      <Show when={dragging()}>
        <div class="drop-overlay">{t("dropHere").replace("{name}", name())}</div>
      </Show>
      <Show when={props.preparing}>
        {(p) => (
          <div class="edit-bar prepare-bar">
            <span class="prepare-name">
              {t("preparing")} {p().name}
            </span>
            <div class="progress">
              <div class="bar" style={{ width: `${p().size > 0 ? (p().bytes / p().size) * 100 : 0}%` }} />
            </div>
            <span class="muted small">
              {formatSize(p().bytes)} / {formatSize(p().size)}
            </span>
          </div>
        )}
      </Show>
      <Show when={editing()}>
        <div class="edit-bar">
          <span>✏️ {t("editing")}</span>
          <span class="muted small">{t("escToCancel")}</span>
          <button type="button" class="ghost" onClick={stopEdit} aria-label={t("cancel")}>
            ✕
          </button>
        </div>
      </Show>
      <Show when={deleting()}>
        {(m) => (
          <Dialog title={t("deleteTitle")} onClose={() => setDeleting(null)}>
            <p class="muted small delete-preview">{m().file?.name ?? m().body}</p>
            <Show when={m().outgoing}>
              <p class="muted small">{t("deleteHint")}</p>
            </Show>
            <div class="delete-actions">
              <Show when={m().outgoing}>
                <button
                  class="danger-solid"
                  onClick={() => {
                    void api.deleteMessage(props.contact.id, m().msgId, true).catch((err) => setError(errorText(err)));
                    setDeleting(null);
                  }}
                >
                  {t("deleteForEveryone")}
                </button>
              </Show>
              <button
                onClick={() => {
                  void api.deleteMessage(props.contact.id, m().msgId, false).catch((err) => setError(errorText(err)));
                  setDeleting(null);
                }}
              >
                {t("deleteForMe")}
              </button>
              <button class="ghost" onClick={() => setDeleting(null)}>
                {t("cancel")}
              </button>
            </div>
          </Dialog>
        )}
      </Show>
      <Show when={viewer()}>{(f) => <Lightbox file={f()} onClose={() => setViewer(null)} />}</Show>
      <form class="composer" onSubmit={send}>
        <button
          type="button"
          class="ghost attach"
          title={t("attach")}
          aria-label={t("attach")}
          disabled={preparing()}
          onClick={() => void sendFiles(() => api.pickAndSendFiles(props.contact.id))}
        >
          {preparing() ? "…" : "📎"}
        </button>
        <input
          ref={input}
          value={text()}
          onInput={(e) => setText(e.currentTarget.value)}
          onKeyDown={onComposerKey}
          placeholder={t("messagePlaceholder")}
          maxLength={16384}
          autofocus
        />
        <button class="primary" type="submit" disabled={!text().trim()}>
          {t("send")}
        </button>
      </form>
      <Show when={error()}>
        <p class="error pad">{error()}</p>
      </Show>
    </>
  );
}

function Lightbox(props: { file: FileTransfer; onClose: () => void }) {
  const onKey = (e: KeyboardEvent) => e.key === "Escape" && props.onClose();
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));
  return (
    <div class="lightbox" role="dialog" aria-modal="true" aria-label={props.file.name} onClick={(e) => e.target === e.currentTarget && props.onClose()}>
      <img src={mediaUrl(props.file.fileId)} alt={props.file.name} />
      <div class="lightbox-bar">
        <span class="lightbox-name" title={props.file.name}>
          {props.file.name} · {formatSize(props.file.size)}
        </span>
        <button onClick={() => void api.openFile(props.file.fileId, true).catch(() => {})}>📂 {t("showInFolder")}</button>
        <button onClick={() => void api.openFile(props.file.fileId, false).catch(() => {})}>{t("open")}</button>
        <button class="ghost" onClick={props.onClose} aria-label={t("close")}>
          ✕
        </button>
      </div>
    </div>
  );
}

/** Inline image, video or audio for a file that is complete on disk. */
function Media(props: { file: FileTransfer; onView: (f: FileTransfer) => void }) {
  const [broken, setBroken] = createSignal(false);
  const src = () => mediaUrl(props.file.fileId);
  return (
    <Show when={!broken()} fallback={<span class="file-status">{t("mediaUnavailable")}</span>}>
      <Switch>
        <Match when={props.file.media === "image"}>
          <button class="media-thumb" onClick={() => props.onView(props.file)} title={props.file.name}>
            <img src={src()} alt={props.file.name} loading="lazy" onError={() => setBroken(true)} />
          </button>
        </Match>
        <Match when={props.file.media === "video"}>
          <video class="media-video" src={src()} controls preload="metadata" onError={() => setBroken(true)} />
        </Match>
        <Match when={props.file.media === "audio"}>
          <audio class="media-audio" src={src()} controls preload="metadata" onError={() => setBroken(true)} />
        </Match>
      </Switch>
    </Show>
  );
}

function FileCard(props: { file: FileTransfer; contactName: string; onView: (f: FileTransfer) => void }) {
  const f = () => props.file;
  const pct = () => (f().size > 0 ? Math.min(100, (f().bytes / f().size) * 100) : 100);
  const act = (p: Promise<void>) => void p.catch(() => {});
  const status = (): string => {
    const file = f();
    if (file.outgoing) {
      if (file.state === "failed") return t("file_unshared");
      if (file.bytes >= file.size && file.size > 0 && file.bytes > 0) return t("file_sent");
      if (file.bytes > 0) return `${t("file_sending")} · ${formatSize(file.bytes)} / ${formatSize(file.size)}`;
      return t("file_shared").replace("{name}", props.contactName);
    }
    switch (file.state) {
      case "downloading":
        return `${formatSize(file.bytes)} / ${formatSize(file.size)}${file.rate > 0 ? ` · ${formatSize(file.rate)}/s` : ""}`;
      case "waiting":
        return `${t("file_waiting").replace("{name}", props.contactName)} · ${Math.floor(pct())} %`;
      case "paused":
        return `${t("file_paused")} · ${Math.floor(pct())} %`;
      default:
        return t(`file_${file.state}` as Key);
    }
  };
  const showBar = () => {
    const file = f();
    return file.outgoing
      ? file.bytes > 0 && file.bytes < file.size && file.state !== "failed"
      : ["downloading", "waiting", "paused"].includes(file.state);
  };

  const showMedia = () => !!f().media && f().openable && f().state !== "failed";
  return (
    <div class="file-card">
      <Show when={showMedia()}>
        <Media file={f()} onView={props.onView} />
      </Show>
      <div class="file-head" classList={{ compact: showMedia() }}>
        <span class="file-icon" aria-hidden="true">
          {f().media === "image" ? "🖼️" : f().media === "video" ? "🎬" : f().media === "audio" ? "🎵" : "📄"}
        </span>
        <div class="file-text">
          <strong class="file-name" title={f().name}>{f().name}</strong>
          <span class="file-size">{formatSize(f().size)}</span>
        </div>
      </div>
      <Show when={showBar()}>
        <div class="progress" role="progressbar" aria-valuenow={Math.floor(pct())} aria-valuemin={0} aria-valuemax={100}>
          <div class="bar" classList={{ idle: f().state !== "downloading" && !f().outgoing }} style={{ width: `${pct()}%` }} />
        </div>
      </Show>
      <span class="file-status">{status()}</span>
      <div class="file-actions">
        <Show when={!f().outgoing}>
          <Show when={f().state === "offered"}>
            <button class="primary" onClick={() => act(api.downloadFile(f().fileId))}>{t("download")}</button>
          </Show>
          <Show when={f().state === "downloading"}>
            <button onClick={() => act(api.pauseFile(f().fileId))}>{t("pause")}</button>
          </Show>
          <Show when={f().state === "paused"}>
            <button onClick={() => act(api.downloadFile(f().fileId))}>{t("resume")}</button>
          </Show>
          <Show when={f().state === "failed"}>
            <button onClick={() => act(api.downloadFile(f().fileId))}>{t("retry")}</button>
          </Show>
          <Show when={["downloading", "paused", "waiting"].includes(f().state)}>
            <button class="ghost" onClick={() => act(api.cancelFile(f().fileId))}>{t("cancel")}</button>
          </Show>
        </Show>
        <Show when={f().openable && (f().outgoing || f().state === "complete")}>
          <Show when={!f().outgoing && !f().media}>
            <button class="primary" onClick={() => act(api.openFile(f().fileId, false))}>{t("open")}</button>
          </Show>
          <button class="ghost" onClick={() => act(api.openFile(f().fileId, true))}>{t("showInFolder")}</button>
        </Show>
        <Show when={f().outgoing && f().state !== "failed" && f().bytes < f().size}>
          <button class="ghost" onClick={() => act(api.cancelFile(f().fileId))}>{t("cancel")}</button>
        </Show>
      </div>
    </div>
  );
}

function InviteDialog(props: { onClose: () => void }) {
  const [invite, setInvite] = createSignal<Invite | null>(null);
  const [copied, setCopied] = createSignal(false);
  onMount(async () => setInvite(await api.myInvite()));

  return (
    <Dialog title={t("inviteTitle")} onClose={props.onClose}>
      <Show when={invite()} fallback={<p class="muted">{t("loading")}</p>}>
        {(inv) => (
          <>
            <p class="muted small">{t("inviteHint")}</p>
            {/* SVG generated locally by the Rust core. */}
            <div class="qr" innerHTML={inv().qrSvg} />
            <textarea class="code" readonly rows={5} value={inv().code} onFocus={(e) => e.currentTarget.select()} />
            <button
              class="primary"
              onClick={async () => {
                await navigator.clipboard.writeText(inv().code);
                setCopied(true);
              }}
            >
              {copied() ? t("copied") : t("copy")}
            </button>
          </>
        )}
      </Show>
    </Dialog>
  );
}

function AddDialog(props: { onClose: () => void; onAdded: (c: Contact) => void }) {
  const [tab, setTab] = createSignal<"name" | "code">("name");
  const [code, setCode] = createSignal("");
  const [error, setError] = createSignal("");

  async function add(e: Event) {
    e.preventDefault();
    setError("");
    try {
      const value = code().trim();
      props.onAdded(tab() === "name" ? await api.addContactByName(value) : await api.addContact(value));
    } catch (err) {
      setError(errorText(err, t));
    }
  }

  return (
    <Dialog title={t("addContact")} onClose={props.onClose}>
      <form onSubmit={add}>
        <div class="tabs">
          <button type="button" classList={{ active: tab() === "name" }} onClick={() => { setTab("name"); setCode(""); }}>
            {t("byName")}
          </button>
          <button type="button" classList={{ active: tab() === "code" }} onClick={() => { setTab("code"); setCode(""); }}>
            {t("byCode")}
          </button>
        </div>
        <Show
          when={tab() === "code"}
          fallback={
            <input
              placeholder={t("usernamePlaceholder")}
              value={code()}
              onInput={(e) => setCode(e.currentTarget.value)}
              maxLength={32}
              autofocus
            />
          }
        >
          <textarea
            class="code"
            rows={5}
            placeholder={t("pasteCode")}
            value={code()}
            onInput={(e) => setCode(e.currentTarget.value)}
            spellcheck={false}
            autofocus
          />
        </Show>
        <Show when={error()}>
          <p class="error">{error()}</p>
        </Show>
        <button class="primary" type="submit" disabled={!code().trim()}>
          {t("add")}
        </button>
      </form>
    </Dialog>
  );
}

function SafetyDialog(props: { contact: Contact; onClose: () => void; onChanged: () => void }) {
  return (
    <Dialog title={`${t("safetyNumber")} · ${props.contact.handle}`} onClose={props.onClose}>
      <p class="muted small">{t("safetyHint")}</p>
      <p class="safety">{props.contact.safetyNumber}</p>
      <button
        class={props.contact.verified ? "" : "primary"}
        onClick={async () => {
          await api.setVerified(props.contact.id, !props.contact.verified);
          props.onChanged();
          props.onClose();
        }}
      >
        {props.contact.verified ? t("unmarkVerified") : t("markVerified")}
      </button>
    </Dialog>
  );
}

function SettingsDialog(props: { server: string | null; onClose: () => void }) {
  const [settings, setSettings] = createSignal<Settings | null>(null);
  const [regEmail, setRegEmail] = createSignal("");
  const [regDiscoverable, setRegDiscoverable] = createSignal(true);
  const [host, setHost] = createSignal(props.server ?? "");
  const [email, setEmail] = createSignal("");
  const [code, setCode] = createSignal("");
  const [codeSent, setCodeSent] = createSignal(false);
  const [error, setError] = createSignal("");
  const [info, setInfo] = createSignal("");
  const [pw, setPw] = createSignal("");
  const [pw2, setPw2] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [confirmDelete, setConfirmDelete] = createSignal(false);

  async function load() {
    const s = await api.getSettings();
    setSettings(s);
    setEmail(s.email ?? "");
  }
  onMount(load);

  async function attempt(action: () => Promise<void>) {
    setError("");
    setInfo("");
    try {
      await action();
    } catch (err) {
      setError(errorText(err, t));
    }
  }

  return (
    <Dialog title={t("settings")} onClose={props.onClose}>
      <Show when={settings()} fallback={<p class="muted">{t("loading")}</p>}>
        {(s) => (
          <>
            <label class="check">
              <input
                type="checkbox"
                checked={s().readReceipts}
                onChange={(e) => setSettings({ ...s(), readReceipts: e.currentTarget.checked })}
              />
              {t("readReceipts")}
            </label>
            <p class="muted small">{t("readReceiptsHint")}</p>

            <label class="check">
              <input
                type="checkbox"
                checked={s().sounds}
                onChange={(e) => setSettings({ ...s(), sounds: e.currentTarget.checked })}
              />
              {t("sounds")}
            </label>

            <h3 class="muted">{t("account")}</h3>
            <Show when={s().accountMissing}>
              <p class="small">
                <strong>{t("registerTitle")}</strong>
              </p>
              <p class="muted small">{t("registerHint")}</p>
              <label>
                {t("email")}
                <input type="email" value={regEmail()} onInput={(e) => setRegEmail(e.currentTarget.value)} autocomplete="email" />
              </label>
              <label class="check">
                <input type="checkbox" checked={regDiscoverable()} onChange={(e) => setRegDiscoverable(e.currentTarget.checked)} />
                {t("discoverable")}
              </label>
              <button
                type="button"
                disabled={busy()}
                onClick={() => attempt(async () => {
                  setBusy(true);
                  try {
                    await api.registerAccount(regEmail().trim() || null, regDiscoverable());
                  } finally {
                    setBusy(false);
                  }
                  setCodeSent(regEmail().trim() !== "");
                  await load();
                })}
              >
                {t("register")}
              </button>
            </Show>
            <Show
              when={s().discoverable !== null}
              fallback={
                <Show when={!s().accountMissing}>
                  <p class="muted small">{t("registryOff")}</p>
                </Show>
              }
            >
              <label class="check">
                <input
                  type="checkbox"
                  checked={s().discoverable ?? true}
                  onChange={(e) => setSettings({ ...s(), discoverable: e.currentTarget.checked })}
                />
                {t("discoverable")}
              </label>
              <p class="muted small">{t("discoverableHint")}</p>
              <label>
                <span>
                  {t("email")}
                  <Show when={s().email}>
                    {" · "}
                    <span classList={{ ok: !!s().emailVerified, warn: !s().emailVerified }}>
                      {s().emailVerified ? t("emailVerified") : t("emailNotVerified")}
                    </span>
                  </Show>
                </span>
                <div class="row">
                  <input type="email" value={email()} onInput={(e) => setEmail(e.currentTarget.value)} />
                  <button
                    type="button"
                    disabled={email().trim() === (s().email ?? "")}
                    onClick={() => attempt(async () => {
                      await api.setEmail(email());
                      setCodeSent(email().trim() !== "");
                      await load();
                    })}
                  >
                    {t("change")}
                  </button>
                </div>
                <small class="muted">{t("emailHint")}</small>
              </label>
              <Show when={s().email && !s().emailVerified}>
                <label>
                  {t("code")}
                  <div class="row">
                    <input value={code()} onInput={(e) => setCode(e.currentTarget.value)} inputMode="numeric" autocomplete="one-time-code" />
                    <button
                      type="button"
                      onClick={() => attempt(async () => {
                        await api.verifyEmail(code());
                        setCode("");
                        await load();
                      })}
                    >
                      {t("verify")}
                    </button>
                  </div>
                  <Show when={codeSent()}>
                    <small class="muted">{t("codeSent")}</small>
                  </Show>
                </label>
              </Show>
            </Show>

            <Show when={s().discoverable !== null}>
              <h3 class="muted">{t("backup")}</h3>
              <p class="muted small">{t("backupHint")}</p>
              <Show
                when={s().backupEnabled}
                fallback={
                  <>
                    <input type="password" placeholder={t("backupPassword")} value={pw()} onInput={(e) => setPw(e.currentTarget.value)} autocomplete="new-password" />
                    <input type="password" placeholder={t("backupConfirm")} value={pw2()} onInput={(e) => setPw2(e.currentTarget.value)} autocomplete="new-password" />
                    <button
                      type="button"
                      disabled={busy() || pw().length < 10}
                      onClick={() => attempt(async () => {
                        if (pw() !== pw2()) throw t("passwordsDiffer");
                        setBusy(true);
                        try {
                          await api.setBackupPassword(pw());
                        } finally {
                          setBusy(false);
                        }
                        setPw("");
                        setPw2("");
                        setInfo(t("backupDone"));
                        await load();
                      })}
                    >
                      {t("backupEnable")}
                    </button>
                  </>
                }
              >
                <p class="small ok">{t("backupOn")}</p>
                <div class="row">
                  <button type="button" disabled={busy()} onClick={() => attempt(async () => {
                    setBusy(true);
                    try {
                      await api.backupNow();
                    } finally {
                      setBusy(false);
                    }
                    setInfo(t("backupDone"));
                  })}>
                    {t("backupNow")}
                  </button>
                  <button type="button" class="ghost" onClick={() => attempt(async () => {
                    await api.disableBackup();
                    await load();
                  })}>
                    {t("backupDisable")}
                  </button>
                </div>
              </Show>
              <button
                type="button"
                class="ghost danger"
                onClick={() => attempt(async () => {
                  if (!confirmDelete()) {
                    setConfirmDelete(true);
                    return;
                  }
                  await api.deleteAccount();
                  setConfirmDelete(false);
                  setInfo(t("deleted"));
                })}
              >
                {confirmDelete() ? t("deleteConfirm") : t("deleteAccount")}
              </button>
            </Show>

            <h3 class="muted">{t("downloads")}</h3>
            <button type="button" onClick={() => attempt(() => api.openDownloadDir())}>
              📂 {t("openDownloadDir")}
            </button>
            <p class="muted small">{t("downloadsHint")}</p>

            <h3 class="muted">{t("server")}</h3>
            <div class="row">
              <input value={host()} onInput={(e) => setHost(e.currentTarget.value)} placeholder="192.168.1.20" />
              <button
                type="button"
                disabled={host().trim() === (props.server ?? "")}
                onClick={() => attempt(() => api.setServer(host().trim() || null))}
              >
                {t("apply")}
              </button>
            </div>
            <p class="muted small">
              {t("serverHint")} {t("restartNote")}
            </p>

            <Show when={info()}>
              <p class="small ok">{info()}</p>
            </Show>
            <Show when={error()}>
              <p class="error">{error()}</p>
            </Show>
            <button
              class="primary"
              onClick={() => attempt(async () => {
                await api.saveSettings(s());
                props.onClose();
              })}
            >
              {t("save")}
            </button>
          </>
        )}
      </Show>
    </Dialog>
  );
}
