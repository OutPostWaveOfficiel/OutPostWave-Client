// Root component.
import { createResource, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { detectLocale, t } from "./i18n";

interface AppInfo {
  version: string;
  os: string;
  arch: string;
}

/** Fetches build info from the Rust backend; `null` when running in a plain browser. */
async function fetchAppInfo(): Promise<AppInfo | null> {
  try {
    return await invoke<AppInfo>("app_info");
  } catch {
    return null;
  }
}

export function App() {
  const locale = detectLocale();
  document.documentElement.lang = locale;
  const [info] = createResource(fetchAppInfo);

  return (
    <>
      <h1>OutpostWave</h1>
      <p>{t(locale, "tagline")}</p>
      <Show when={!info.loading}>
        <p class="muted">
          {info() ? `v${info()!.version} · ${info()!.os} / ${info()!.arch}` : t(locale, "noBackend")}
        </p>
      </Show>
    </>
  );
}
