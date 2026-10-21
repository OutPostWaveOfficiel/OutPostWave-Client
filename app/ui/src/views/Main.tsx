// Main screen: contact list + conversation.
import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { api, errorText, type Contact, type Invite, type Message, type Settings } from "../api";
import { Dialog } from "../components/Dialog";
import { chime, setBadge } from "../notify";
import { locale, t, type Key } from "../i18n";

const timeFmt = new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" });

export function Main(props: { handle: string; os: string; server: string | null }) {
  const [contacts, setContacts] = createSignal<Contact[]>([]);
  const [sounds, setSounds] = createSignal(true);
  const loadPrefs = () => api.getPrefs().then((p) => setSounds(p.sounds)).catch(() => {});
  const [selected, setSelected] = createSignal<number | null>(null);
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
}) {
  const [text, setText] = createSignal("");
  const [error, setError] = createSignal("");
  let list: HTMLDivElement | undefined;

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
            <div class="bubble" classList={{ out: m.outgoing }}>
              <p>{m.body}</p>
              <span class="meta">
                {timeFmt.format(new Date(m.sentAt))}
                <Show when={m.outgoing}>{` · ${t(`status_${m.status}` as Key)}`}</Show>
              </span>
            </div>
          )}
        </For>
      </div>
      <form class="composer" onSubmit={send}>
        <input
          value={text()}
          onInput={(e) => setText(e.currentTarget.value)}
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
