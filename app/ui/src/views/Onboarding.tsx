// Account creation / restoration, then display of the recovery phrase.
import { createSignal, For, Show } from "solid-js";
import { api, errorText } from "../api";
import { t } from "../i18n";

export function Onboarding(props: { defaultDevice: string; onDone: () => void }) {
  const [mode, setMode] = createSignal<"create" | "restore">("create");
  const [name, setName] = createSignal("");
  const [device, setDevice] = createSignal(props.defaultDevice);
  const [phrase, setPhrase] = createSignal("");
  const [words, setWords] = createSignal<string[] | null>(null);
  const [confirmed, setConfirmed] = createSignal(false);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  async function submit(e: Event) {
    e.preventDefault();
    setError("");
    setBusy(true);
    try {
      if (mode() === "create") {
        setWords(await api.createIdentity(name(), device()));
      } else {
        await api.restoreIdentity(phrase(), name(), device());
        props.onDone();
      }
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main class="onboarding">
      <Show
        when={words()}
        fallback={
          <form class="card" onSubmit={submit}>
            <h1>{t("welcome")}</h1>
            <p class="muted">{t("tagline")}</p>
            <div class="tabs">
              <button type="button" classList={{ active: mode() === "create" }} onClick={() => setMode("create")}>
                {t("create")}
              </button>
              <button type="button" classList={{ active: mode() === "restore" }} onClick={() => setMode("restore")}>
                {t("restore")}
              </button>
            </div>
            <label>
              {t("name")}
              <input value={name()} onInput={(e) => setName(e.currentTarget.value)} required minLength={3} maxLength={32} autofocus />
              <small class="muted">{t("nameHint")}</small>
            </label>
            <label>
              {t("deviceName")}
              <input value={device()} onInput={(e) => setDevice(e.currentTarget.value)} required maxLength={64} />
            </label>
            <Show when={mode() === "restore"}>
              <label>
                {t("phrase")}
                <textarea rows={4} value={phrase()} onInput={(e) => setPhrase(e.currentTarget.value)} required spellcheck={false} autocomplete="off" />
              </label>
            </Show>
            <Show when={error()}>
              <p class="error">{error()}</p>
            </Show>
            <button class="primary" type="submit" disabled={busy()}>
              {t("continue")}
            </button>
          </form>
        }
      >
        {(list) => (
          <div class="card">
            <h1>{t("phraseTitle")}</h1>
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
                props.onDone();
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
