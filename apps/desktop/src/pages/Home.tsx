import { Copy, KeyRound, Monitor, RefreshCw, Star, Wifi, WifiOff } from "lucide-react";
import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { Button, StatusDot, TextInput, Toggle } from "../components/ui";
import { groupId, normalizeIdInput } from "../lib/format";
import { describeService } from "../lib/status";
import { useApp } from "../lib/store";

function CopyButton({ text, label }: { text: string; label: string }) {
  const { t } = useTranslation();
  const [done, setDone] = useState(false);
  return (
    <Button
      size="sm"
      variant="ghost"
      aria-label={label}
      title={label}
      icon={<Copy size={15} />}
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => {
          setDone(true);
          window.setTimeout(() => setDone(false), 1500);
        });
      }}
    >
      {done ? t("common.copied") : t("common.copy")}
    </Button>
  );
}

function ServerStatus() {
  const { t } = useTranslation();
  const { state } = useApp();
  // Being connected is not enough: the service only counts as ready once this device has its ID.
  const effective = state.server.state === "online" && !state.id ? ({ state: "connecting" } as const) : state.server;
  const status = describeService(effective, state.settings?.incomingEnabled ?? true);
  const Icon = status.icon === "offline" ? WifiOff : Wifi;
  return (
    <div className="flex items-center gap-2 text-[13px]" role="status" aria-live="polite">
      <StatusDot tone={status.tone} pulse={status.icon === "online" && status.tone === "ok"} />
      <Icon size={14} aria-hidden className="text-muted" />
      <span>{t(`status.${status.key}`)}</span>
    </div>
  );
}

function ThisDevice() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  const ttl = state.passwordTtl ? Math.round(state.passwordTtl / 60) : null;
  return (
    <section aria-labelledby="this-device" className="flex flex-col rounded-2xl border border-line bg-surface p-6 shadow-card">
      <h2 id="this-device" className="mb-5 flex items-center gap-2 text-[13px] font-semibold uppercase tracking-wide text-muted">
        <Monitor size={15} aria-hidden /> {t("home.thisDevice")}
      </h2>

      <div className="mb-1 text-[13px] text-muted">{t("home.yourId")}</div>
      <div className="mb-4 flex items-center justify-between gap-2">
        <div data-selectable className="ltr tabular text-[2rem] font-semibold leading-tight" aria-live="polite">
          {state.id ?? <span className="text-muted">{t("home.waiting")}</span>}
        </div>
        {state.id && <CopyButton text={state.id.replace(/\s/g, "")} label={t("home.copyId")} />}
      </div>

      <div className="mb-1 text-[13px] text-muted">{t("home.tempPassword")}</div>
      <div className="mb-2 flex items-center justify-between gap-2">
        <div data-selectable className="ltr tabular text-2xl font-semibold tracking-[0.18em]">{state.password}</div>
        <div className="flex gap-1">
          <CopyButton text={state.password} label={t("home.copyPassword")} />
        </div>
      </div>
      <div className="mb-4 text-[13px] text-muted">{ttl ? t("home.expiresIn", { minutes: ttl }) : t("home.neverExpires")}</div>
      <Button className="self-start" icon={<RefreshCw size={15} />} onClick={() => void act.regeneratePassword()}>
        {t("home.generateNew")}
      </Button>

      <div className="mt-6 border-t border-line pt-4">
        <p className="mb-3 text-[13px] text-muted">{t("home.shareHint")}</p>
        <ServerStatus />
        <Toggle
          label={t("home.allowIncoming")}
          checked={state.settings?.incomingEnabled ?? true}
          onChange={(v) => void act.saveSettings({ incomingEnabled: v })}
        />
      </div>
    </section>
  );
}

function RemoteDevice() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  const [id, setId] = useState("");
  const [password, setPassword] = useState("");
  const [unattended, setUnattended] = useState(false);
  const [elevated, setElevated] = useState(false);
  const [busy, setBusy] = useState(false);
  const canSubmit = id.length === 9 && password.length > 0 && !busy && describeService(state.server, true).online;

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setBusy(true);
    await act.connect(id, password, unattended, elevated);
    setBusy(false);
  }

  const err = state.connectError;
  const entries = state.book.entries;
  const recents = state.book.recents.map((r) => ({ ...r, entry: entries.find((e) => e.id === r.id) }));
  const favorites = entries.filter((e) => e.favorite);

  const pick = (deviceId: string) => {
    setId(deviceId);
    document.getElementById("remote-password")?.focus();
  };

  const row = (deviceId: string, name: string | undefined, fav: boolean) => {
    const online = state.presence[deviceId];
    return (
      <li key={deviceId}>
        <button
          className="flex w-full items-center justify-between gap-3 rounded-lg px-3 py-2 text-start transition hover:bg-surface-2"
          onClick={() => pick(deviceId)}
        >
          <span className="flex min-w-0 items-center gap-2">
            {fav && <Star size={14} className="shrink-0 fill-warn text-warn" aria-hidden />}
            <span className="truncate font-medium">{name ?? groupId(deviceId)}</span>
            {name && <span className="ltr tabular text-[13px] text-muted">{groupId(deviceId)}</span>}
          </span>
          <span className="flex shrink-0 items-center gap-1.5 text-[13px] text-muted">
            <StatusDot tone={online ? "ok" : "muted"} />
            {online ? t("home.online") : t("home.offlineDevice")}
          </span>
        </button>
      </li>
    );
  };

  return (
    <section aria-labelledby="remote-device" className="flex flex-col rounded-2xl border border-line bg-surface p-6 shadow-card">
      <h2 id="remote-device" className="mb-5 flex items-center gap-2 text-[13px] font-semibold uppercase tracking-wide text-muted">
        <KeyRound size={15} aria-hidden /> {t("home.remoteDevice")}
      </h2>
      <form onSubmit={submit} className="flex flex-col gap-3" noValidate>
        <label className="text-[13px] text-muted" htmlFor="remote-id">{t("home.enterId")}</label>
        <TextInput
          id="remote-id"
          value={groupId(id)}
          onChange={(e) => setId(normalizeIdInput(e.target.value))}
          placeholder={t("home.idPlaceholder")}
          inputMode="numeric"
          autoComplete="off"
          dir="ltr"
          className="tabular h-11 text-lg"
          aria-invalid={err === "invalidId" || err === "offline"}
        />
        <TextInput
          id="remote-password"
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          placeholder={unattended ? t("settings.unattendedPassword") : t("home.passwordPlaceholder")}
          autoComplete="off"
          dir="ltr"
          className="h-11"
          aria-label={t("home.passwordPlaceholder")}
        />
        <label className="flex cursor-pointer items-center gap-2 text-[13px]">
          <input type="checkbox" checked={unattended} onChange={(e) => setUnattended(e.target.checked)} className="size-4 accent-[var(--accent)]" />
          {t("home.unattendedToggle")}
        </label>
        <fieldset className="flex flex-wrap gap-x-4 gap-y-1 text-[13px]">
          <legend className="sr-only">{t("access.level")}</legend>
          <label className="flex cursor-pointer items-center gap-2">
            <input type="radio" name="access-level" checked={!elevated} onChange={() => setElevated(false)} className="size-4 accent-[var(--accent)]" />
            {t("access.standard")}
          </label>
          <label className="flex cursor-pointer items-center gap-2">
            <input type="radio" name="access-level" checked={elevated} onChange={() => setElevated(true)} className="size-4 accent-[var(--accent)]" />
            {t("access.full")}
          </label>
        </fieldset>
        {err && (
          <p role="alert" className="rounded-lg bg-danger/10 px-3 py-2 text-[13px] text-danger">
            {t(`errors.${err}`, { defaultValue: t("errors.generic"), app: state.boot?.brand.name })}
          </p>
        )}
        <Button type="submit" variant="primary" size="lg" disabled={!canSubmit}>
          {busy ? t("home.connecting") : t("home.connect")}
        </Button>
      </form>

      <div className="mt-6 min-h-0 flex-1 overflow-auto">
        {favorites.length > 0 && (
          <>
            <h3 className="mb-1 text-[13px] font-semibold text-muted">{t("home.favorites")}</h3>
            <ul className="mb-3">{favorites.map((f) => row(f.id, f.name, true))}</ul>
          </>
        )}
        <h3 className="mb-1 text-[13px] font-semibold text-muted">{t("home.recent")}</h3>
        {recents.length === 0 ? (
          <p className="px-3 py-2 text-[13px] text-muted">{t("home.noRecent")}</p>
        ) : (
          <ul>{recents.map((r) => row(r.id, r.entry?.name, r.entry?.favorite ?? false))}</ul>
        )}
      </div>
    </section>
  );
}

export default function Home() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  return (
    <div className="mx-auto flex h-full max-w-5xl flex-col p-6">
      {state.session && (
        <button
          onClick={() => act.navigate("session")}
          className="anim-pop mb-4 flex items-center justify-between rounded-xl border border-accent/40 bg-accent-soft px-4 py-3 text-start font-medium text-accent transition hover:bg-accent/20"
        >
          <span>{t("home.sessionOpen")}</span>
          <span className="ltr tabular text-[13px]">{groupId(state.session.target)}</span>
        </button>
      )}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-5 lg:grid-cols-2">
        <ThisDevice />
        <RemoteDevice />
      </div>
    </div>
  );
}
