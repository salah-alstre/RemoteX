import { Globe, KeyRound, Lock, PlugZap, Rocket, Sparkles } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Select, Toggle, cx } from "../components/ui";
import { useApp } from "../lib/store";
import type { Settings } from "../lib/types";

const steps = ["welcome", "language", "privacy", "incoming", "unattended", "finish"] as const;

/** Short guided setup shown once. Every choice maps to a real setting. */
export default function FirstRun() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  const app = state.boot?.brand.name ?? "";
  const [i, setI] = useState(0);
  const [incoming, setIncoming] = useState(true);
  const [autostart, setAutostart] = useState(false);
  const [busy, setBusy] = useState(false);
  const step = steps[i] ?? "welcome";
  const s = state.settings as Settings;
  const Icon = { welcome: Sparkles, language: Globe, privacy: Lock, incoming: PlugZap, unattended: KeyRound, finish: Rocket }[step];

  async function finish() {
    setBusy(true);
    await act.saveSettings({ firstRunDone: true, incomingEnabled: incoming, startWithWindows: autostart, unattendedEnabled: false });
  }

  return (
    <div className="anim-fade fixed inset-0 z-[60] grid place-items-center bg-bg p-6">
      <div className="anim-pop w-full max-w-lg rounded-3xl border border-line bg-surface p-8 shadow-card" role="dialog" aria-modal="true" aria-labelledby="fr-title">
        <div className="mb-6 flex items-center justify-between">
          <div className="grid size-12 place-items-center rounded-2xl bg-accent-soft text-accent"><Icon size={24} aria-hidden /></div>
          <span className="text-[13px] text-muted" aria-live="polite">{t("firstRun.step", { current: i + 1, total: steps.length })}</span>
        </div>

        <h1 id="fr-title" className="mb-2 text-2xl font-semibold">
          {step === "welcome" && t("firstRun.welcomeTitle", { app })}
          {step === "language" && t("firstRun.languageTitle")}
          {step === "privacy" && t("firstRun.privacyTitle")}
          {step === "incoming" && t("firstRun.incomingTitle")}
          {step === "unattended" && t("firstRun.unattendedTitle")}
          {step === "finish" && t("firstRun.doneTitle")}
        </h1>

        <div className="mb-8 min-h-32 text-muted">
          {step === "welcome" && <p>{t("firstRun.welcomeBody")}</p>}
          {step === "language" && (
            <Select value={s.language} onChange={(e) => void act.saveSettings({ language: e.target.value as Settings["language"] })} className="w-full text-fg" aria-label={t("settings.language_label")}>
              <option value="system">{t("settings.langSystem")}</option>
              <option value="en">{t("settings.langEn")}</option>
              <option value="ar">{t("settings.langAr")}</option>
            </Select>
          )}
          {step === "privacy" && (
            <ul className="space-y-2 text-fg">
              {[1, 2, 3].map((n) => <li key={n} className="flex gap-2"><Lock size={15} className="mt-1 shrink-0 text-ok" aria-hidden />{t(`settings.privacyBody${n}`)}</li>)}
            </ul>
          )}
          {step === "incoming" && (
            <>
              <p className="mb-2">{t("firstRun.incomingBody")}</p>
              <div className="text-fg"><Toggle label={t("firstRun.allowIncoming")} checked={incoming} onChange={setIncoming} /></div>
            </>
          )}
          {step === "unattended" && <p>{t("firstRun.unattendedBody")}</p>}
          {step === "finish" && (
            <>
              <p className="mb-2">{t("firstRun.doneBody")}</p>
              <div className="text-fg"><Toggle label={t("firstRun.startWithWindows", { app })} checked={autostart} onChange={setAutostart} /></div>
            </>
          )}
        </div>

        <div className="flex items-center justify-between">
          <Button variant="ghost" onClick={() => setI((n) => Math.max(0, n - 1))} className={cx(i === 0 && "invisible")}>{t("common.back")}</Button>
          {step === "finish" ? (
            <Button variant="primary" size="lg" disabled={busy} onClick={() => void finish()}>{t("common.finish")}</Button>
          ) : (
            <Button variant="primary" size="lg" onClick={() => setI((n) => n + 1)}>{step === "unattended" ? t("firstRun.unattendedOff") : t("common.next")}</Button>
          )}
        </div>
      </div>
    </div>
  );
}
