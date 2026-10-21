// Main screen: contact list + conversation.
import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { api, errorText, type Contact, type Invite, type Message } from "../api";
import { Dialog } from "../components/Dialog";
import { locale, t, type Key } from "../i18n";

const timeFmt = new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" });

export function Main(props: { handle: string }) {
  const [contacts, setContacts] = createSignal<Contact[]>([]);
  const [selected, setSelected] = createSignal<number | null>(null);
  // Messages per contact, oldest first.
  const [messages, setMessages] = createStore<Record<number, Message[]>>({});
  const [dialog, setDialog] = createSignal<"invite" | "add" | "safety" | null>(null);

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

  onMount(async () => {
    await loadContacts();
    const unlisten = await api.onEvent((e) => {
      switch (e.type) {
        case "message":
          if (messages[e.contact]) {
            setMessages(e.contact, produce((list) => {
              if (!list.some((m) => m.msgId === e.message.msgId)) list.push(e.message);
            }));
          }
          break;
        case "status":
          setMessages(e.contact, (m) => m.msgId === e.msgId, "status", e.status);
          break;
        case "presence":
          setContacts((list) => list.map((c) => (c.id === e.contact ? { ...c, online: e.online } : c)));
          break;
        case "contacts":
          void loadContacts();
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
  });

  return (
    <div class="layout">
      <aside class="sidebar">
        <div class="me">
          <strong>{props.handle}</strong>
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
  const [code, setCode] = createSignal("");
  const [error, setError] = createSignal("");

  async function add(e: Event) {
    e.preventDefault();
    setError("");
    try {
      props.onAdded(await api.addContact(code()));
    } catch (err) {
      setError(errorText(err));
    }
  }

  return (
    <Dialog title={t("addContact")} onClose={props.onClose}>
      <form onSubmit={add}>
        <textarea
          class="code"
          rows={5}
          placeholder={t("pasteCode")}
          value={code()}
          onInput={(e) => setCode(e.currentTarget.value)}
          spellcheck={false}
          autofocus
        />
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
