// Modal dialog. Closes on Escape or backdrop click.
import { onCleanup, onMount, type JSX } from "solid-js";
import { t } from "../i18n";

export function Dialog(props: { title: string; onClose: () => void; children: JSX.Element }) {
  const onKey = (e: KeyboardEvent) => e.key === "Escape" && props.onClose();
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));
  return (
    <div class="backdrop" onClick={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="dialog" role="dialog" aria-modal="true" aria-label={props.title}>
        <header>
          <h2>{props.title}</h2>
          <button class="ghost" onClick={props.onClose} aria-label={t("close")}>
            ✕
          </button>
        </header>
        {props.children}
      </div>
    </div>
  );
}
