import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Field, Section, Select, TextInput, Toggle, cx } from "../components/ui";
import { PermissionToggles } from "../components/Permissions";
import { api } from "../lib/api";
import { describeService } from "../lib/status";
import { useApp } from "../lib/store";
import type { Settings as S } from "../lib/types";

const sections = ["general", "connection", "display", "security", "privacy", "unattended", "devices", "files", "language", "appearance", "advanced", "about"] as const;
type SectionKey = (typeof sections)[number];

function useSettings() {
  const { state, act } = useApp();
  const s = state.settings as S;
  const set = useCallback((patch: Partial<S>) => void act.saveSettings(patch), [act]);
  return { s, set, state, act };
}

function General() {
  const { t } = useTranslation();
  const { s, set, state } = useSettings();
  return (
    <>
      <Section title={t("settings.general")}>
        <Toggle label={t("settings.startWithWindows")} checked={s.startWithWindows} onChange={(v) => set({ startWithWindows: v })} />
        <Toggle label={t("settings.minimizeToTray")} checked={s.minimizeToTray} onChange={(v) => set({ minimizeToTray: v })} />
        <Toggle label={t("settings.launchMinimized")} checked={s.launchMinimized} onChange={(v) => set({ launchMinimized: v })} />
        <Field label={t("settings.closeBehavior")}>
          <Select value={s.closeBehavior} onChange={(e) => set({ closeBehavior: e.target.value as S["closeBehavior"] })}>
            <option value="tray">{t("settings.closeTray")}</option>
            <option value="exit">{t("settings.closeExit")}</option>
          </Select>
        </Field>
        <Toggle label={t("settings.checkUpdates")} checked={s.checkUpdates} onChange={(v) => set({ checkUpdates: v })} />
        <Field label={t("settings.deviceName")} hint={t("settings.deviceNameHint")}>
          <TextInput defaultValue={s.deviceName} placeholder={state.boot?.computerName} maxLength={64} onBlur={(e) => e.target.value !== s.deviceName && set({ deviceName: e.target.value })} />
        </Field>
      </Section>
    </>
  );
}

function Connection() {
  const { t } = useTranslation();
  const { s, set, state } = useSettings();
  return (
    <>
      <Section title={t("settings.connection")}>
        <Field label={t("settings.preset")}>
          <Select value={s.preset} onChange={(e) => set({ preset: e.target.value as S["preset"] })}>
            {(["auto", "best", "balanced", "lowLatency", "lowBandwidth"] as const).map((p) => (
              <option key={p} value={p}>{t(`quality.${p}`)}</option>
            ))}
          </Select>
        </Field>
        <Field label={t("settings.preferredCodec")}>
          <Select value={s.codec} onChange={(e) => set({ codec: e.target.value as S["codec"] })}>
            <option value="auto">{t("common.auto")}</option>
            <option value="h264">{t("quality.h264")}</option>
            <option value="h265">{t("quality.h265")}</option>
          </Select>
        </Field>
        <Field label={t("settings.fps")}>
          <Select value={s.fps} onChange={(e) => set({ fps: Number(e.target.value) as S["fps"] })}>
            <option value={0}>{t("common.auto")}</option>
            {[15, 30, 60].map((f) => <option key={f} value={f}>{f}</option>)}
          </Select>
        </Field>
        <Field label={t("settings.resolution")}>
          <Select value={s.resolution} onChange={(e) => set({ resolution: e.target.value })}>
            <option value="auto">{t("common.auto")}</option>
            <option value="native">{t("quality.native")}</option>
            <option value="1080">1080p</option>
            <option value="720">720p</option>
            <option value="480">480p</option>
          </Select>
        </Field>
        <Field label={t("settings.bitrate")}>
          <Select value={s.bitrateKbps} onChange={(e) => set({ bitrateKbps: Number(e.target.value) })}>
            <option value={0}>{t("common.auto")}</option>
            {[1000, 2500, 5000, 10000, 20000, 40000].map((b) => <option key={b} value={b}>{b / 1000} Mbps</option>)}
          </Select>
        </Field>
        <Toggle label={t("settings.hardwareAcceleration")} checked={s.hardwareAcceleration} onChange={(v) => set({ hardwareAcceleration: v })} />
        <Toggle label={t("settings.adaptiveQuality")} checked={s.adaptiveQuality} onChange={(v) => set({ adaptiveQuality: v })} />
        <Toggle label={t("settings.lowBandwidth")} checked={s.lowBandwidth} onChange={(v) => set({ lowBandwidth: v })} />
        <Toggle label={t("settings.performanceMode")} hint={t("settings.performanceHint")} checked={s.performanceMode} onChange={(v) => set({ performanceMode: v })} />
        <Toggle label={t("settings.preferDirect")} checked={s.preferDirect} onChange={(v) => set({ preferDirect: v })} />
        <Toggle label={t("settings.relayFallback")} checked={s.relayFallback} onChange={(v) => set({ relayFallback: v })} />
      </Section>
      <Section title={t("about.encoders")}>
        {state.boot?.encoders.map((e) => (
          <div key={`${e.codec}-${e.name}`} className="flex items-center justify-between py-2">
            <span>{e.name}</span>
            <span className="text-[13px] text-muted">{e.codec} · {e.hardware ? t("about.hardware") : t("about.software")}</span>
          </div>
        ))}
      </Section>
    </>
  );
}

function Display() {
  const { t } = useTranslation();
  const { s, set } = useSettings();
  return (
    <Section title={t("settings.display")}>
      <Toggle label={t("settings.showRemoteCursor")} checked={s.showRemoteCursor} onChange={(v) => set({ showRemoteCursor: v })} />
      <Field label={t("settings.cursorPrediction")} hint={t("settings.cursorPredictionHint")}>
        <Select value={s.cursorPrediction} onChange={(e) => set({ cursorPrediction: e.target.value as S["cursorPrediction"] })}>
          <option value="auto">{t("settings.cursorAuto")}</option>
          <option value="on">{t("settings.cursorOn")}</option>
          <option value="off">{t("settings.cursorOff")}</option>
        </Select>
      </Field>
      <Field label={t("settings.scaleMode")}>
        <Select value={s.scaleMode} onChange={(e) => set({ scaleMode: e.target.value as S["scaleMode"] })}>
          <option value="fit">{t("toolbar.fit")}</option>
          <option value="original">{t("toolbar.original")}</option>
          <option value="stretch">{t("toolbar.stretch")}</option>
        </Select>
      </Field>
      <Toggle label={t("settings.clipboardSync")} checked={s.clipboardSync} onChange={(v) => set({ clipboardSync: v })} />
      <Toggle label={t("settings.clipboardImages")} checked={s.clipboardImages} onChange={(v) => set({ clipboardImages: v })} />
    </Section>
  );
}

function Security() {
  const { t } = useTranslation();
  const { s, set, act } = useSettings();
  const [log, setLog] = useState<string[]>([]);
  const refresh = useCallback(() => void api.securityLog(50).then(setLog), []);
  useEffect(refresh, [refresh]);
  return (
    <>
      <Section title={t("settings.security")}>
        <Toggle label={t("settings.incoming")} hint={t("settings.approvalAlways")} checked={s.incomingEnabled} onChange={(v) => set({ incomingEnabled: v })} />
        <Field label={t("settings.tempExpiry")}>
          <Select value={s.tempPasswordExpiryMins} onChange={(e) => set({ tempPasswordExpiryMins: Number(e.target.value) })}>
            <option value={0}>{t("settings.expiryNever")}</option>
            {[5, 15, 30, 60].map((m) => <option key={m} value={m}>{t("settings.expiryMins", { minutes: m })}</option>)}
          </Select>
        </Field>
        <Toggle label={t("settings.rotateAfter")} checked={s.rotateAfterSession} onChange={(v) => set({ rotateAfterSession: v })} />
        <div className="py-2.5">
          <div className="mb-2 font-medium">{t("settings.defaultPermissions")}</div>
          <PermissionToggles value={s.defaultPermissions} onChange={(p) => set({ defaultPermissions: p })} />
        </div>
        <div className="py-2.5">
          <Button variant="danger" size="sm" onClick={() => void api.revokeAll().then((trusted) => act.setBook({ trusted }))}>{t("settings.revokeAll")}</Button>
        </div>
      </Section>
      <Section title={t("settings.securityLog")}>
        <div className="flex items-center justify-between py-2">
          <span className="text-[13px] text-muted">{log.length === 0 ? t("settings.noLog") : ""}</span>
          <Button size="sm" onClick={refresh}>{t("settings.refresh")}</Button>
        </div>
        {log.length > 0 && (
          <pre data-selectable className="ltr max-h-56 overflow-auto py-2 text-[12px] leading-5 text-muted">{log.join("\n")}</pre>
        )}
      </Section>
    </>
  );
}

function Privacy() {
  const { t } = useTranslation();
  return (
    <Section title={t("settings.privacy")}>
      {[1, 2, 3].map((n) => <p key={n} className="py-3">{t(`settings.privacyBody${n}`)}</p>)}
    </Section>
  );
}

function Unattended() {
  const { t } = useTranslation();
  const { s, set, state } = useSettings();
  const [pw, setPw] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [isSet, setIsSet] = useState(state.boot?.unattendedPasswordSet ?? false);
  return (
    <Section title={t("settings.unattended")}>
      <p className="rounded-lg bg-warn/10 my-3 px-3 py-2 text-[13px] text-warn">{t("settings.unattendedWarn")}</p>
      <Toggle label={t("settings.unattendedEnable")} checked={s.unattendedEnabled} disabled={!isSet} onChange={(v) => set({ unattendedEnabled: v })} />
      <Field label={t("settings.unattendedPassword")} hint={isSet ? t("settings.unattendedIsSet") : t("settings.unattendedNotSet")}>
        <div className="flex gap-2">
          <TextInput type="password" value={pw} onChange={(e) => setPw(e.target.value)} autoComplete="new-password" dir="ltr" />
          <Button
            variant="primary"
            disabled={pw.length < 8}
            onClick={() => {
              setErr(null);
              api.setUnattendedPassword(pw).then(() => { setPw(""); setIsSet(true); }).catch((e) => setErr(String(e)));
            }}
          >
            {t("settings.unattendedSet")}
          </Button>
        </div>
        {err && <p role="alert" className="mt-1 text-[13px] text-danger">{t(`errors.${err}`, { defaultValue: t("errors.generic") })}</p>}
      </Field>
      {isSet && (
        <div className="py-2.5">
          <Button size="sm" variant="danger" onClick={() => void api.clearUnattendedPassword().then(() => { setIsSet(false); set({ unattendedEnabled: false }); })}>
            {t("settings.unattendedClear")}
          </Button>
        </div>
      )}
      <Toggle label={t("settings.anyDevice")} hint={t("settings.anyDeviceHint")} checked={s.unattendedAnyDevice} onChange={(v) => set({ unattendedAnyDevice: v })} />
      <Toggle label={t("settings.unattendedElevated")} hint={t("settings.unattendedElevatedHint")} checked={s.unattendedElevated} disabled={!s.unattendedEnabled} onChange={(v) => set({ unattendedElevated: v })} />
    </Section>
  );
}

function DevicesSection() {
  const { t } = useTranslation();
  const { act } = useSettings();
  return (
    <Section title={t("settings.devices")}>
      <div className="py-3">
        <p className="mb-3 text-muted">{t("devices.trustedDesc")}</p>
        <Button onClick={() => act.navigate("devices")}>{t("nav.devices")}</Button>
      </div>
    </Section>
  );
}

function FolderPicker({ value, onPick, placeholder }: { value: string; onPick: (dir: string) => void; placeholder?: string }) {
  const { t } = useTranslation();
  return (
    <div className="flex gap-2">
      <TextInput readOnly value={value} placeholder={placeholder} dir="ltr" data-selectable />
      <Button onClick={() => void openDialog({ directory: true }).then((d) => typeof d === "string" && onPick(d))}>{t("settings.chooseFolder")}</Button>
    </div>
  );
}

function Files() {
  const { t } = useTranslation();
  const { s, set, state } = useSettings();
  return (
    <Section title={t("settings.files")}>
      <Field label={t("settings.downloadDir")}>
        <FolderPicker value={s.downloadDir} placeholder={state.boot?.downloadsDir} onPick={(d) => set({ downloadDir: d })} />
      </Field>
      <Toggle label={t("settings.ask")} checked={s.askBeforeReceiving} onChange={(v) => set({ askBeforeReceiving: v })} />
      <Field label={t("settings.maxTransfers")}>
        <Select value={s.maxTransfers} onChange={(e) => set({ maxTransfers: Number(e.target.value) })}>
          {[1, 2, 3, 4, 6, 8].map((n) => <option key={n} value={n}>{n}</option>)}
        </Select>
      </Field>
      <Toggle label={t("settings.openAfter")} checked={s.openDirAfterTransfer} onChange={(v) => set({ openDirAfterTransfer: v })} />
      <div className="py-2.5"><Button size="sm" onClick={() => void api.openFolder("downloads")}>{t("settings.openFolder")}</Button></div>
    </Section>
  );
}

function Language() {
  const { t } = useTranslation();
  const { s, set } = useSettings();
  return (
    <Section title={t("settings.language")}>
      <Field label={t("settings.language_label")}>
        <Select value={s.language} onChange={(e) => set({ language: e.target.value as S["language"] })}>
          <option value="system">{t("settings.langSystem")}</option>
          <option value="en">{t("settings.langEn")}</option>
          <option value="ar">{t("settings.langAr")}</option>
        </Select>
      </Field>
    </Section>
  );
}

function Appearance() {
  const { t } = useTranslation();
  const { s, set } = useSettings();
  return (
    <Section title={t("settings.appearance")}>
      <Field label={t("settings.theme")}>
        <div role="radiogroup" className="flex gap-1 rounded-lg bg-surface-2 p-1">
          {(["system", "light", "dark"] as const).map((v) => (
            <button
              key={v}
              role="radio"
              aria-checked={s.theme === v}
              onClick={() => set({ theme: v })}
              className={cx("rounded-md px-3 py-1 text-[13px] font-medium transition", s.theme === v ? "bg-surface shadow-sm" : "text-muted hover:text-fg")}
            >
              {t(`settings.theme${v[0]?.toUpperCase()}${v.slice(1)}`)}
            </button>
          ))}
        </div>
      </Field>
      <Field label={t("settings.uiScale")}>
        <div className="flex items-center gap-3">
          <input type="range" min={80} max={160} step={10} value={s.uiScale} onChange={(e) => set({ uiScale: Number(e.target.value) })} aria-label={t("settings.uiScale")} className="accent-[var(--accent)]" />
          <span className="tabular w-12 text-end">{s.uiScale}%</span>
        </div>
      </Field>
    </Section>
  );
}

function Advanced() {
  const { t } = useTranslation();
  return (
    <>
      <Section title={t("settings.advanced")}>
        <div className="flex flex-wrap gap-2 py-3">
          <Button onClick={() => void api.openFolder("logs")}>{t("settings.openLogsFolder")}</Button>
          <Button onClick={() => void api.openFolder("data")}>{t("settings.dataFolder")}</Button>
        </div>
      </Section>
    </>
  );
}

function About() {
  const { t } = useTranslation();
  const { state } = useApp();
  const b = state.boot;
  const [status, setStatus] = useState<{ kind: "idle" | "checking" | "current" | "available" | "failed"; version?: string }>({ kind: "idle" });
  const [installing, setInstalling] = useState(false);
  if (!b) return null;

  async function checkUpdates() {
    setStatus({ kind: "checking" });
    try {
      const update = await check();
      setStatus(update ? { kind: "available", version: update.version } : { kind: "current" });
    } catch {
      setStatus({ kind: "failed" });
    }
  }

  async function install() {
    setInstalling(true);
    try {
      const update = await check();
      await update?.downloadAndInstall();
      await relaunch();
    } catch {
      setStatus({ kind: "failed" });
      setInstalling(false);
    }
  }

  const rows: [string, string][] = [
    [t("about.version"), b.version],
    [t("about.build"), b.build],
    [t("about.server"), t(`status.${describeService(state.server, true).short}`)],
  ];
  return (
    <>
      <Section title={`${b.brand.name}`}>
        {rows.map(([k, v]) => (
          <div key={k} className="flex justify-between py-2.5"><span className="text-muted">{k}</span><span className="ltr" data-selectable>{v}</span></div>
        ))}
        <div className="flex justify-between py-2.5"><span className="text-muted">{t("about.website")}</span><a className="text-accent underline" href={b.brand.website} target="_blank" rel="noreferrer">{b.brand.website}</a></div>
        <div className="flex justify-between py-2.5"><span className="text-muted">{t("about.github")}</span><a className="text-accent underline" href={b.brand.github} target="_blank" rel="noreferrer">{b.brand.github}</a></div>
        <div className="flex justify-between py-2.5"><span className="text-muted">{t("about.support")}</span><span className="ltr" data-selectable>{b.brand.supportEmail}</span></div>
        <div className="flex flex-wrap items-center gap-3 py-3">
          <Button variant="primary" onClick={() => void checkUpdates()} disabled={status.kind === "checking"}>
            {status.kind === "checking" ? t("about.checking") : t("about.checkUpdates")}
          </Button>
          {status.kind === "current" && <span role="status">{t("about.upToDate")}</span>}
          {status.kind === "failed" && <span role="alert" className="text-danger">{t("about.updateFailed")}</span>}
          {status.kind === "available" && (
            <>
              <span role="status">{t("about.updateAvailable", { version: status.version })}</span>
              <Button variant="primary" disabled={installing} onClick={() => void install()}>{t("about.install")}</Button>
            </>
          )}
        </div>
      </Section>
      <Section title={t("about.licenses")}>
        <p className="py-3 text-muted">{t("about.licensesBody")}</p>
        <p className="pb-3 text-[13px] text-muted">{t("about.notice", { company: b.brand.company })}</p>
      </Section>
    </>
  );
}

const panels: Record<SectionKey, () => React.JSX.Element | null> = {
  general: General, connection: Connection, display: Display, security: Security, privacy: Privacy, unattended: Unattended,
  devices: DevicesSection, files: Files, language: Language, appearance: Appearance, advanced: Advanced, about: About,
};

export default function Settings() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  const current = (sections as readonly string[]).includes(state.settingsSection) ? (state.settingsSection as SectionKey) : "general";
  const Panel = panels[current];
  if (!state.settings) return null;
  return (
    <div className="flex h-full">
      <nav aria-label={t("settings.title")} className="w-52 shrink-0 overflow-auto border-e border-line p-3">
        <h1 className="mb-2 px-3 text-lg font-semibold">{t("settings.title")}</h1>
        <ul className="flex flex-col gap-0.5">
          {sections.map((k) => (
            <li key={k}>
              <button
                onClick={() => act.navigate("settings", k)}
                aria-current={current === k ? "page" : undefined}
                className={cx("w-full rounded-lg px-3 py-1.5 text-start transition", current === k ? "bg-accent-soft font-medium text-accent" : "hover:bg-surface-2")}
              >
                {t(`settings.${k}`)}
              </button>
            </li>
          ))}
        </ul>
      </nav>
      <div className="min-w-0 flex-1 overflow-auto p-6">
        <div className="mx-auto max-w-2xl"><Panel /></div>
      </div>
    </div>
  );
}
