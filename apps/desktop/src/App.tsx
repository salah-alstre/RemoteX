import { Home as HomeIcon, Settings as SettingsIcon, Users } from "lucide-react";
import { Component, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { relaunch } from "@tauri-apps/plugin-process";
import { Button, Toast, cx } from "./components/ui";
import { HostBanner, IncomingDialog } from "./components/HostUi";
import { api } from "./lib/api";
import { AppProvider, useApp, type Route } from "./lib/store";
import Devices from "./pages/Devices";
import FirstRun from "./pages/FirstRun";
import Home from "./pages/Home";
import Settings from "./pages/Settings";
import Viewer from "./session/Viewer";

const nav: { route: Exclude<Route, "session">; icon: typeof HomeIcon; label: string }[] = [
  { route: "home", icon: HomeIcon, label: "nav.home" },
  { route: "devices", icon: Users, label: "nav.devices" },
  { route: "settings", icon: SettingsIcon, label: "nav.settings" },
];

function Shell() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  if (!state.ready || !state.settings) return null;

  const inSession = state.route === "session" && state.session;
  return (
    <div className="flex h-full flex-col">
      <HostBanner />
      <div className="flex min-h-0 flex-1">
        {!inSession && (
          <nav aria-label={t("nav.main")} className="flex w-[4.5rem] shrink-0 flex-col items-center gap-1 border-e border-line bg-surface py-3">
            {nav.map(({ route, icon: Icon, label }) => {
              const active = state.route === route;
              return (
                <button
                  key={route}
                  onClick={() => act.navigate(route)}
                  aria-current={active ? "page" : undefined}
                  className={cx("flex w-16 flex-col items-center gap-0.5 rounded-xl px-1 py-2 text-[11px] font-medium transition", active ? "bg-accent-soft text-accent" : "text-muted hover:bg-surface-2 hover:text-fg")}
                >
                  <Icon size={20} aria-hidden />
                  {t(label)}
                </button>
              );
            })}
          </nav>
        )}
        <main className="min-w-0 flex-1 overflow-hidden">
          {inSession ? <Viewer /> : state.route === "devices" ? <Devices /> : state.route === "settings" ? <Settings /> : <Home />}
        </main>
      </div>

      {state.incoming[0] && <IncomingDialog key={state.incoming[0].requestId} request={state.incoming[0]} />}
      {!state.settings.firstRunDone && <FirstRun />}

      <div className="pointer-events-none fixed bottom-4 end-4 z-[70] flex flex-col gap-2" aria-live="polite">
        {state.toasts.map((toast) => (
          <Toast key={toast.key} message={toast.message} tone={toast.tone} onDone={() => act.dismissToast(toast.key)} />
        ))}
      </div>
    </div>
  );
}

/** Last line of defence: the UI never dies silently. */
class ErrorBoundary extends Component<{ children: ReactNode; t: (k: string) => string }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  componentDidCatch(error: Error) {
    console.error("UI error", error.message);
  }
  render() {
    const { t } = this.props;
    if (!this.state.failed) return this.props.children;
    return (
      <div className="grid h-full place-items-center p-8">
        <div className="max-w-md rounded-2xl border border-line bg-surface p-8 text-center shadow-card" role="alert">
          <h1 className="mb-2 text-xl font-semibold">{t("crash.title")}</h1>
          <p className="mb-6 text-muted">{t("crash.body")}</p>
          <div className="flex justify-center gap-2">
            <Button variant="primary" onClick={() => void relaunch()}>{t("crash.restart")}</Button>
            <Button onClick={() => void api.openFolder("logs")}>{t("crash.logs")}</Button>
          </div>
        </div>
      </div>
    );
  }
}

export default function App() {
  const { t } = useTranslation();
  return (
    <ErrorBoundary t={t}>
      <AppProvider>
        <Shell />
      </AppProvider>
    </ErrorBoundary>
  );
}
