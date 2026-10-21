// Account creation, restoration (recovery phrase) and email recovery, then the recovery phrase.
import { createSignal, For, Match, Show, Switch } from "solid-js";
import { api, errorText } from "../api";
import { t } from "../i18n";

type Mode = "create" | "restore" | "recover";

export function Onboarding(props: {
  defaultDevice: string;
  registry: boolean;
  server: string | null;
  onServerChanged: () => void;
  onDone: () => void;
}) {
  const [editServer, setEditServer] = createSignal(false);
  const [host, setHost] = createSignal(props.server ?? "");
  const [mode, setMode] = createSignal<Mode>("create");
  const [name, setName] = createSignal("");
  const [device, setDevice] = createSignal(props.defaultDevice);
  const [email, setEmail] = createSignal("");
  const [discoverable, setDiscoverable] = createSignal(true);
  const [phrase, setPhrase] = createSignal("");
  const [code, setCode] = createSignal("");
  const [codeSent, setCodeSent] = createSignal(false);
  const [words, setWords] = createSignal<string[] | null>(null);
  const [recovered, setRecovered] = createSignal(false);
  const [confirmed, setConfirmed] = createSignal(false);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  // After a recovery or a restore: offer to restore the encrypted backup.
  const [restoreStep, setRestoreStep] = createSignal(false);
  const [password, setPassword] = createSignal("");
  const [restoredCount, setRestoredCount] = createSignal<number | null>(null);

  async function run(action: () => Promise<void>) {
    setError("");
    setBusy(true);
    try {
      await action();
    } catch (err) {
      setError(errorText(err, t));
    } finally {
      setBusy(false);
    }
  }

  function submit(e: Event) {
    e.preventDefault();
    void run(async () => {
      switch (mode()) {
        case "create":
          setWords(await api.createIdentity(name(), device(), email().trim() || null, discoverable()));
          break;
        case "restore":
          await api.restoreIdentity(phrase(), name(), device());
          if (props.registry) setRestoreStep(true);
          else props.onDone();
          break;
        case "recover":
          if (!codeSent()) {
            await api.recoveryStart(name());
            setCodeSent(true);
          } else {
            setWords(await api.recoveryComplete(name(), code(), device()));
            setRecovered(true);
          }
          break;
      }
    });
  }

  const switchTo = (m: Mode) => {
    setMode(m);
    setError("");
    setCodeSent(false);
  };

  return (
    <main class="onboarding">
      <Show when={restoreStep()}>
        <form
          class="card"
          onSubmit={(e) => {
            e.preventDefault();
            void run(async () => {
              setRestoredCount(await api.restoreBackup(password()));
              setPassword("");
            });
          }}
        >
          <h1>{t("restoreTitle")}</h1>
          <Show
            when={restoredCount() === null}
            fallback={
              <>
                <p>
                  {restoredCount()} {t("restored")}
                </p>
                <button class="primary" type="button" onClick={() => props.onDone()}>
                  {t("continue")}
                </button>
              </>
            }
          >
            <p class="muted small">{t("restoreHint")}</p>
            <label>
              {t("backupPassword")}
              <input type="password" value={password()} onInput={(e) => setPassword(e.currentTarget.value)} required autocomplete="current-password" autofocus />
            </label>
            <Show when={error()}>
              <p class="error">{error()}</p>
            </Show>
            <button class="primary" type="submit" disabled={busy()}>
              {t("restore_action")}
            </button>
            <button type="button" class="ghost link" onClick={() => props.onDone()}>
              {t("skip")}
            </button>
          </Show>
        </form>
      </Show>
      <Show
        when={!restoreStep() && words()}
        fallback={
          <Show when={!restoreStep()}>
          <form class="card" onSubmit={submit}>
            <Show
              when={mode() !== "recover"}
              fallback={
                <>
                  <h1>{t("recoverTitle")}</h1>
                  <button type="button" class="ghost back" onClick={() => switchTo("create")}>
                    ← {t("back")}
                  </button>
                </>
              }
            >
              <h1>{t("welcome")}</h1>
              <p class="muted">{t("tagline")}</p>
              <div class="tabs">
                <button type="button" classList={{ active: mode() === "create" }} onClick={() => switchTo("create")}>
                  {t("create")}
                </button>
                <button type="button" classList={{ active: mode() === "restore" }} onClick={() => switchTo("restore")}>
                  {t("restore")}
                </button>
              </div>
            </Show>

            <label>
              {t("name")}
              <input value={name()} onInput={(e) => setName(e.currentTarget.value)} required minLength={3} maxLength={32} autofocus disabled={codeSent()} />
              <Show when={mode() === "create"}>
                <small class="muted">{t("nameHint")}</small>
              </Show>
            </label>

            <Switch>
              <Match when={mode() === "create"}>
                <label>
                  {t("deviceName")}
                  <input value={device()} onInput={(e) => setDevice(e.currentTarget.value)} required maxLength={64} />
                </label>
                <Show when={props.registry} fallback={<p class="muted small">{t("registryOff")}</p>}>
                  <label>
                    {t("email")}
                    <input type="email" value={email()} onInput={(e) => setEmail(e.currentTarget.value)} autocomplete="email" />
                    <small class="muted">{t("emailHint")}</small>
                  </label>
                  <label class="check">
                    <input type="checkbox" checked={discoverable()} onChange={(e) => setDiscoverable(e.currentTarget.checked)} />
                    {t("discoverable")}
                  </label>
                  <small class="muted">{t("discoverableHint")}</small>
                </Show>
              </Match>
              <Match when={mode() === "restore"}>
                <label>
                  {t("deviceName")}
                  <input value={device()} onInput={(e) => setDevice(e.currentTarget.value)} required maxLength={64} />
                </label>
                <label>
                  {t("phrase")}
                  <textarea rows={4} value={phrase()} onInput={(e) => setPhrase(e.currentTarget.value)} required spellcheck={false} autocomplete="off" />
                </label>
              </Match>
              <Match when={mode() === "recover" && codeSent()}>
                <p class="muted small">{t("codeSent")}</p>
                <label>
                  {t("code")}
                  <input value={code()} onInput={(e) => setCode(e.currentTarget.value)} required inputMode="numeric" autocomplete="one-time-code" autofocus />
                </label>
                <label>
                  {t("deviceName")}
                  <input value={device()} onInput={(e) => setDevice(e.currentTarget.value)} required maxLength={64} />
                </label>
              </Match>
            </Switch>

            <Show when={error()}>
              <p class="error">{error()}</p>
            </Show>
            <button class="primary" type="submit" disabled={busy()}>
              {mode() === "recover" && !codeSent() ? t("sendCode") : t("continue")}
            </button>
            <Show when={props.registry && mode() !== "recover"}>
              <button type="button" class="ghost link" onClick={() => switchTo("recover")}>
                {t("recover")}
              </button>
            </Show>
            <div class="server-line muted small">
              <Show
                when={editServer()}
                fallback={
                  <>
                    {t("server")} : {props.server ?? t("serverNone")}{" · "}
                    <button type="button" class="ghost inline" onClick={() => setEditServer(true)}>
                      {t("change_server")}
                    </button>
                  </>
                }
              >
                <div class="row">
                  <input value={host()} onInput={(e) => setHost(e.currentTarget.value)} placeholder="192.168.1.20" />
                  <button
                    type="button"
                    onClick={() => void run(async () => {
                      await api.setServer(host().trim() || null);
                      setEditServer(false);
                      props.onServerChanged();
                    })}
                  >
                    {t("apply")}
                  </button>
                </div>
                <small>{t("serverHint")}</small>
              </Show>
            </div>
          </form>
          </Show>
        }
      >
        {(list) => (
          <div class="card">
            <h1>{t("phraseTitle")}</h1>
            <Show when={recovered()}>
              <p class="muted">{t("newPhraseInfo")}</p>
            </Show>
            <p class="warning">{t("phraseWarning")}</p>
            <ol class="words">
              <For each={list()}>{(w) => <li>{w}</li>}</For>
            </ol>
            <label class="check">
              <input type="checkbox" checked={confirmed()} onChange={(e) => setConfirmed(e.currentTarget.checked)} />
              {t("phraseConfirm")}
            </label>
            <button
              class="primary"
              disabled={!confirmed()}
              onClick={() => {
                setWords(null); // Do not keep the phrase in memory.
                if (recovered()) setRestoreStep(true);
                else props.onDone();
              }}
            >
              {t("continue")}
            </button>
          </div>
        )}
      </Show>
    </main>
  );
}
