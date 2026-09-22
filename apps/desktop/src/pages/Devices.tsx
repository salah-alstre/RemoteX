import { Pencil, Plus, ShieldOff, Star, Trash2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, IconButton, Modal, Select, StatusDot, TextInput } from "../components/ui";
import { api } from "../lib/api";
import { groupId, normalizeIdInput } from "../lib/format";
import { useApp } from "../lib/store";
import type { AddressEntry } from "../lib/types";

function useDate() {
  const { i18n } = useTranslation();
  return (ms: number | null) => (ms ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" }).format(ms) : null);
}

function EntryDialog({ initial, onClose }: { initial: Partial<AddressEntry>; onClose: () => void }) {
  const { t } = useTranslation();
  const { act } = useApp();
  const [id, setId] = useState(initial.id ?? "");
  const [name, setName] = useState(initial.name ?? "");
  const [favorite, setFavorite] = useState(initial.favorite ?? false);
  const [group, setGroup] = useState<"my" | "other">(initial.group ?? "my");
  const [error, setError] = useState<string | null>(null);
  const editing = Boolean(initial.id);

  async function save() {
    try {
      const entries = await api.upsertEntry({ id, name, favorite, group });
      act.setBook({ entries });
      onClose();
    } catch (e) {
      setError(typeof e === "string" ? e : "generic");
    }
  }

  return (
    <Modal
      title={editing ? t("common.edit") : t("devices.add")}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button variant="primary" disabled={id.length !== 9 || !name.trim()} onClick={() => void save()}>
            {t("devices.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <label className="text-[13px] text-muted">
          {t("devices.id")}
          <TextInput value={groupId(id)} onChange={(e) => setId(normalizeIdInput(e.target.value))} disabled={editing} dir="ltr" inputMode="numeric" className="tabular mt-1" />
        </label>
        <label className="text-[13px] text-muted">
          {t("devices.alias")}
          <TextInput value={name} onChange={(e) => setName(e.target.value)} maxLength={64} className="mt-1" />
        </label>
        <label className="text-[13px] text-muted">
          {t("devices.group")}
          <Select value={group} onChange={(e) => setGroup(e.target.value as "my" | "other")} className="mt-1 w-full">
            <option value="my">{t("devices.my")}</option>
            <option value="other">{t("devices.other")}</option>
          </Select>
        </label>
        <label className="flex items-center gap-2">
          <input type="checkbox" checked={favorite} onChange={(e) => setFavorite(e.target.checked)} className="size-4 accent-[var(--accent)]" />
          {t("devices.favorite")}
        </label>
        {error && <p role="alert" className="text-[13px] text-danger">{t(`errors.${error}`, { defaultValue: t("errors.generic") })}</p>}
      </div>
    </Modal>
  );
}

function Trusted() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  const fmt = useDate();
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirmAll, setConfirmAll] = useState(false);
  const list = state.book.trusted;

  return (
    <div>
      <div className="mb-3 flex items-center justify-between gap-3">
        <p className="text-[13px] text-muted">{t("devices.trustedDesc")}</p>
        <Button variant="danger" size="sm" disabled={list.length === 0} icon={<ShieldOff size={14} />} onClick={() => setConfirmAll(true)}>
          {t("devices.revokeAll")}
        </Button>
      </div>
      {list.length === 0 ? (
        <p className="rounded-xl border border-line bg-surface p-6 text-center text-muted">{t("devices.noTrusted")}</p>
      ) : (
        <div className="overflow-hidden rounded-xl border border-line bg-surface">
          <table className="w-full text-start">
            <thead className="bg-surface-2 text-[13px] text-muted">
              <tr>
                {["name", "id", "lastConnection", "added", "actions"].map((h) => (
                  <th key={h} className="px-4 py-2 text-start font-medium">{t(`devices.${h}`)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {list.map((d) => (
                <tr key={d.id} className="border-t border-line">
                  <td className="px-4 py-2.5">
                    {renaming === d.id ? (
                      <TextInput
                        autoFocus
                        value={draft}
                        onChange={(e) => setDraft(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            void api.renameTrusted(d.id, draft).then((trusted) => act.setBook({ trusted }));
                            setRenaming(null);
                          }
                          if (e.key === "Escape") setRenaming(null);
                        }}
                        className="h-8"
                      />
                    ) : (
                      d.name
                    )}
                  </td>
                  <td className="ltr tabular px-4 py-2.5">{groupId(d.id)}</td>
                  <td className="px-4 py-2.5 text-muted">{fmt(d.lastConnectionMs) ?? t("common.never")}</td>
                  <td className="px-4 py-2.5 text-muted">{fmt(d.addedMs)}</td>
                  <td className="px-4 py-2.5">
                    <div className="flex gap-1">
                      <Button size="sm" variant="primary" onClick={() => void act.connect(d.id, "", true)} disabled={!state.settings?.unattendedEnabled} title={t("home.unattendedToggle")}>
                        {t("devices.connect")}
                      </Button>
                      <IconButton label={t("common.rename")} onClick={() => { setRenaming(d.id); setDraft(d.name); }}>
                        <Pencil size={15} />
                      </IconButton>
                      <IconButton label={t("devices.revoke")} onClick={() => void api.revoke(d.id).then((trusted) => act.setBook({ trusted }))}>
                        <ShieldOff size={15} />
                      </IconButton>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {confirmAll && (
        <Modal
          title={t("devices.revokeAll")}
          onClose={() => setConfirmAll(false)}
          footer={
            <>
              <Button onClick={() => setConfirmAll(false)}>{t("common.cancel")}</Button>
              <Button variant="danger" onClick={() => void api.revokeAll().then((trusted) => { act.setBook({ trusted }); setConfirmAll(false); })}>
                {t("devices.revokeAll")}
              </Button>
            </>
          }
        >
          {t("devices.revokeAllConfirm")}
        </Modal>
      )}
    </div>
  );
}

function AddressBook() {
  const { t } = useTranslation();
  const { state, act } = useApp();
  const fmt = useDate();
  const [editing, setEditing] = useState<Partial<AddressEntry> | null>(null);
  const [tab, setTab] = useState<"favorites" | "recent" | "my">("favorites");
  const { entries, recents } = state.book;
  const shown =
    tab === "favorites" ? entries.filter((e) => e.favorite) : tab === "my" ? entries.filter((e) => e.group === "my") : recents.map((r) => entries.find((e) => e.id === r.id) ?? { id: r.id, name: groupId(r.id), favorite: false, group: "other" as const, addedMs: r.lastMs, lastConnectedMs: r.lastMs });

  return (
    <div>
      <div className="mb-3 flex items-center justify-between">
        <div role="tablist" className="flex gap-1 rounded-lg bg-surface-2 p-1">
          {(["favorites", "recent", "my"] as const).map((k) => (
            <button
              key={k}
              role="tab"
              aria-selected={tab === k}
              onClick={() => setTab(k)}
              className={`rounded-md px-3 py-1 text-[13px] font-medium transition ${tab === k ? "bg-surface shadow-sm" : "text-muted hover:text-fg"}`}
            >
              {t(k === "my" ? "devices.myDevices" : `devices.${k}`)}
            </button>
          ))}
        </div>
        <Button size="sm" variant="primary" icon={<Plus size={14} />} onClick={() => setEditing({})}>{t("devices.add")}</Button>
      </div>
      {shown.length === 0 ? (
        <p className="rounded-xl border border-line bg-surface p-6 text-center text-muted">{t("devices.noEntries")}</p>
      ) : (
        <ul className="divide-y divide-line overflow-hidden rounded-xl border border-line bg-surface">
          {shown.map((e) => {
            const known = entries.some((x) => x.id === e.id);
            const online = state.presence[e.id];
            return (
              <li key={e.id} className="flex items-center justify-between gap-3 px-4 py-3">
                <div className="flex min-w-0 items-center gap-3">
                  {e.favorite && <Star size={15} className="fill-warn text-warn" aria-label={t("devices.favorite")} />}
                  <div className="min-w-0">
                    <div className="truncate font-medium">{e.name}</div>
                    <div className="ltr tabular text-[13px] text-muted">{groupId(e.id)}</div>
                  </div>
                </div>
                <div className="flex items-center gap-4">
                  <span className="hidden items-center gap-1.5 text-[13px] text-muted sm:flex">
                    <StatusDot tone={online ? "ok" : "muted"} />
                    {online ? t("home.online") : t("home.offlineDevice")}
                  </span>
                  <span className="hidden text-[13px] text-muted md:block">{fmt(e.lastConnectedMs) ?? t("common.never")}</span>
                  <div className="flex gap-1">
                    <Button size="sm" onClick={() => { act.navigate("home"); }} title={t("home.enterId")}>{t("devices.connect")}</Button>
                    <IconButton label={known ? t("common.edit") : t("home.saveDevice")} onClick={() => setEditing(e)}>
                      {known ? <Pencil size={15} /> : <Plus size={15} />}
                    </IconButton>
                    {known && (
                      <IconButton label={t("common.delete")} onClick={() => void api.removeEntry(e.id).then((list) => act.setBook({ entries: list }))}>
                        <Trash2 size={15} />
                      </IconButton>
                    )}
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {editing && <EntryDialog initial={editing} onClose={() => setEditing(null)} />}
    </div>
  );
}

export default function Devices() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"trusted" | "book">("trusted");
  return (
    <div className="mx-auto h-full max-w-5xl overflow-auto p-6">
      <h1 className="mb-4 text-xl font-semibold">{t("devices.title")}</h1>
      <div role="tablist" className="mb-4 flex gap-6 border-b border-line">
        {(["trusted", "book"] as const).map((k) => (
          <button
            key={k}
            role="tab"
            aria-selected={tab === k}
            onClick={() => setTab(k)}
            className={`-mb-px border-b-2 pb-2 font-medium transition ${tab === k ? "border-accent text-accent" : "border-transparent text-muted hover:text-fg"}`}
          >
            {t(k === "trusted" ? "devices.trusted" : "devices.addressBook")}
          </button>
        ))}
      </div>
      {tab === "trusted" ? <Trusted /> : <AddressBook />}
    </div>
  );
}
