import { MessageSquare, ShieldAlert, ShieldCheck } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../lib/api";
import { formatBytes, groupId } from "../lib/format";
import { useApp } from "../lib/store";
import { availablePermissions, PermissionToggles } from "./Permissions";
import { ChatPanel, TransferRow } from "../session/panels";
import { Button, IconButton, Modal, Toggle, cx } from "./ui";
import type { IncomingRequest, Permissions } from "../lib/types";

function Countdown({ seconds }: { seconds: number }) {
  const { t } = useTranslation();
  const [left, setLeft] = useState(seconds);
  useEffect(() => {
    const id = window.setInterval(() => setLeft((s) => Math.max(0, s - 1)), 1000);
    return () => window.clearInterval(id);
  }, []);
  return <span className="tabular text-[13px] text-muted">{t("incoming.expiresIn", { seconds: left })}</span>;
}

/** Consent dialog: the host chooses exactly which permissions to grant. */
export function IncomingDialog({ request }: { request: IncomingRequest }) {
  const { t, i18n } = useTranslation();
  const { state, act } = useApp();
  // Elevated control is never pre-selected: the owner has to choose it deliberately.
  const [perms, setPerms] = useState<Permissions>({ ...request.requested, elevated: false });
  const [serviceReady, setServiceReady] = useState<boolean | null>(null);
  useEffect(() => {
    if (request.requested.elevated) void api.elevationAvailable().then(setServiceReady, () => setServiceReady(false));
  }, [request.requested.elevated]);
  const [trust, setTrust] = useState(false);
  const [requestedAt] = useState(() => Date.now());
  const anything = availablePermissions.some((k) => perms[k]);
  const alreadyTrusted = state.book.trusted.some((d) => d.id === request.viewerId);
  const time = new Intl.DateTimeFormat(i18n.language, { timeStyle: "medium" }).format(requestedAt);

  return (
    <Modal
      title={t("incoming.title")}
      dismissible={false}
      footer={
        <>
          <Button size="lg" onClick={() => void act.respondIncoming(request, null, false)}>{t("incoming.reject")}</Button>
          <Button size="lg" variant="primary" disabled={!anything} title={anything ? undefined : t("incoming.nothing")} onClick={() => void act.respondIncoming(request, perms, trust && !alreadyTrusted)}>
            {t("incoming.accept")}
          </Button>
        </>
      }
    >
      <div className="mb-4 flex items-start gap-3">
        <div className="grid size-11 shrink-0 place-items-center rounded-full bg-accent-soft text-accent"><ShieldCheck size={22} aria-hidden /></div>
        <div className="min-w-0">
          <p className="font-medium" dir="auto">{t("incoming.wantsToConnect", { name: request.viewerName })}</p>
          <Countdown seconds={request.timeoutSecs} />
        </div>
      </div>
      <dl className="mb-4 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-[13px]">
        <dt className="text-muted">{t("incoming.device")}</dt><dd dir="auto">{request.viewerName}</dd>
        <dt className="text-muted">{t("incoming.id")}</dt><dd className="ltr tabular">{groupId(request.viewerId)}</dd>
        <dt className="text-muted">{t("incoming.time")}</dt><dd className="ltr tabular">{time}</dd>
        <dt className="text-muted">{t("incoming.network")}</dt><dd>{request.sameNetwork ? t("incoming.sameNetwork") : t("incoming.internet")}</dd>
      </dl>
      {request.requested.elevated && (
        <fieldset className="mb-4 rounded-xl border border-line p-3">
          <legend className="px-1 font-medium">{t("access.level")}</legend>
          <label className="flex cursor-pointer items-start gap-2 py-1">
            <input type="radio" name="access" className="mt-1 accent-[var(--accent)]" checked={!perms.elevated} onChange={() => setPerms({ ...perms, elevated: false })} />
            <span><span className="font-medium">{t("access.standard")}</span><span className="block text-[13px] text-muted">{t("access.standardHint")}</span></span>
          </label>
          <label className={cx("flex items-start gap-2 py-1", serviceReady ? "cursor-pointer" : "cursor-not-allowed opacity-60")}>
            <input type="radio" name="access" className="mt-1 accent-[var(--accent)]" disabled={!serviceReady} checked={perms.elevated} onChange={() => setPerms({ ...perms, elevated: true })} />
            <span>
              <span className="flex items-center gap-1 font-medium"><ShieldAlert size={15} aria-hidden />{t("access.full")}</span>
              <span className="block text-[13px] text-muted">{serviceReady === false ? t("access.unavailable") : t("access.fullHint")}</span>
            </span>
          </label>
        </fieldset>
      )}
      <div className="mb-2 font-medium">{t("incoming.permissions")}</div>
      <PermissionToggles value={perms} onChange={setPerms} only={request.requested} />
      {!alreadyTrusted && <div className="mt-2"><Toggle label={t("incoming.trust")} checked={trust} onChange={setTrust} /></div>}
    </Modal>
  );
}

/** Always-visible indicator while someone is connected to this computer, with live permission control. */
export function HostBanner() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  const host = state.host;
  const [open, setOpen] = useState(false);
  const [chat, setChat] = useState(false);
  const [handled, setHandled] = useState<number[]>([]);
  if (!host) return null;
  const transfers = Object.values(host.transfers).sort((a, b) => b.id - a.id).slice(0, 4);
  return (
    <div className="border-b border-warn/40 bg-warn/10" role="region" aria-label={t("host.active")}>
      <div className="flex flex-wrap items-center gap-3 px-4 py-2">
        <span aria-hidden className="size-2.5 animate-pulse rounded-full bg-danger" />
        <div className="min-w-0 flex-1">
          <span className="font-semibold">{t("host.active")}</span>
          {host.permissions.elevated && (
            <span className="ms-2 inline-flex items-center gap-1 rounded-full bg-danger/15 px-2 py-0.5 text-[12px] font-semibold text-danger" role="status">
              <ShieldAlert size={13} aria-hidden />{t("elevated.indicator")}
            </span>
          )}
          <span className="ms-2 text-[13px] text-muted" dir="auto">{t("host.with", { name: host.viewerName, id: groupId(host.viewerId) })}</span>
        </div>
        <Button size="sm" onClick={() => setOpen((v) => !v)} aria-expanded={open}>{t("host.controls")}</Button>
        <IconButton label={t("host.chat")} active={chat} onClick={() => setChat((v) => !v)}><MessageSquare size={17} /></IconButton>
        <Button size="sm" variant="danger" onClick={() => void api.hostKick()}>{t("host.end")}</Button>
      </div>
      {open && (
        <div className="anim-fade max-w-md px-4 pb-3">
          <PermissionToggles value={host.permissions} onChange={(p) => void api.hostSetPermissions(p)} />
        </div>
      )}
      {host.fileRequests.filter((f) => !handled.includes(f.id)).map((f) => (
        <Modal key={f.id} title={t("host.fileAskTitle")} dismissible={false} footer={<>
          <Button onClick={() => { void api.hostApproveFile(f.id, false); setHandled((h) => [...h, f.id]); }}>{t("host.deny")}</Button>
          <Button variant="primary" onClick={() => { void api.hostApproveFile(f.id, true); setHandled((h) => [...h, f.id]); }}>{t("host.allow")}</Button>
        </>}>
          <span dir="auto">{t("host.fileAskBody", { name: f.name, size: formatBytes(f.size) })}</span>
        </Modal>
      ))}
      {transfers.length > 0 && <ul className="max-w-md space-y-2 px-4 pb-3">{transfers.map((tr) => <TransferRow key={tr.id} tr={tr} />)}</ul>}
      {chat && (
        <div className="anim-fade h-72 max-w-md border-t border-line bg-surface">
          <ChatPanel messages={host.chat} onSend={(text) => void api.hostChat(text)} onClear={act.clearChat} />
        </div>
      )}
    </div>
  );
}
