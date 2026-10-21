// OutpostWave UI entry point.
// Framework-agnostic placeholder until the SolidJS vs Svelte decision is made.
import { invoke } from "@tauri-apps/api/core";
import { t, detectLocale } from "./i18n";

interface AppInfo {
  version: string;
  os: string;
  arch: string;
}

async function render(): Promise<void> {
  const root = document.querySelector<HTMLElement>("#app");
  if (!root) return;
  const locale = detectLocale();
  document.documentElement.lang = locale;

  const title = document.createElement("h1");
  title.textContent = "OutpostWave";
  const tagline = document.createElement("p");
  tagline.textContent = t(locale, "tagline");
  const info = document.createElement("p");
  info.className = "muted";

  try {
    const app = await invoke<AppInfo>("app_info");
    info.textContent = `v${app.version} · ${app.os} / ${app.arch}`;
  } catch {
    // Running in a plain browser (no Tauri backend).
    info.textContent = t(locale, "noBackend");
  }
  root.replaceChildren(title, tagline, info);
}

void render();
