export function formatBytes(n: number, locale?: string): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  const digits = i === 0 || v >= 100 ? 0 : v >= 10 ? 1 : 2;
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(v)} ${units[i]}`;
}

export function formatRate(bps: number, locale?: string): string {
  return `${formatBytes(bps, locale)}/s`;
}

export function formatDuration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  const mm = String(m).padStart(h > 0 ? 2 : 1, "0");
  const rr = String(r).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${rr}` : `${mm}:${rr}`;
}

export function formatMbps(kbps: number, locale?: string): string {
  return new Intl.NumberFormat(locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(kbps / 1000);
}

/** Groups a nine-digit id as `583 294 814`. */
export function groupId(digits: string): string {
  const d = digits.replace(/\D/g, "");
  return [d.slice(0, 3), d.slice(3, 6), d.slice(6, 9)].filter(Boolean).join(" ");
}

const ARABIC_INDIC = /[٠-٩]/g;
const PERSIAN = /[۰-۹]/g;

/** Accepts ids typed on Arabic keyboards (Arabic-Indic digits) and returns ASCII digits only. */
export function normalizeIdInput(input: string): string {
  return input
    .replace(ARABIC_INDIC, (c) => String(c.charCodeAt(0) - 0x0660))
    .replace(PERSIAN, (c) => String(c.charCodeAt(0) - 0x06f0))
    .replace(/\D/g, "")
    .slice(0, 9);
}
