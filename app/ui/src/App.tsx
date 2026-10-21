// Root component: onboarding until an identity exists, then the main screen.
import { createResource, Match, Switch } from "solid-js";
import { api } from "./api";
import { t } from "./i18n";
import { Main } from "./views/Main";
import { Onboarding } from "./views/Onboarding";

const OS_NAMES: Record<string, string> = {
  windows: "PC Windows",
  macos: "Mac",
  linux: "PC Linux",
  android: "Android",
  ios: "iPhone",
};

export function App() {
  document.documentElement.lang = navigator.language.toLowerCase().startsWith("fr") ? "fr" : "en";
  const [info, { refetch: refetchInfo }] = createResource(() => api.appInfo().catch(() => null));
  const [handle, { refetch }] = createResource(() => api.identityStatus().catch(() => undefined));

  return (
    <Switch fallback={<div class="center-screen muted">{t("loading")}</div>}>
      <Match when={handle() === undefined && !handle.loading}>
        <div class="center-screen muted">{t("noBackend")}</div>
      </Match>
      <Match when={handle() === null}>
        <Onboarding
          defaultDevice={OS_NAMES[info()?.os ?? ""] ?? "PC"}
          registry={info()?.registry ?? false}
          server={info()?.server ?? null}
          onServerChanged={() => void refetchInfo()}
          onDone={() => void refetch()}
        />
      </Match>
      <Match when={handle()}>
        {(h) => <Main handle={h()} os={info()?.os ?? ""} server={info()?.server ?? null} />}
      </Match>
    </Switch>
  );
}
