import { describe, expect, it } from "vitest";
import ar from "./ar.json";
import en from "./en.json";
import { applyLanguage, directionOf, resolveLanguage } from "./index";

type Tree = { [k: string]: string | Tree };

function flatten(t: Tree, prefix = ""): Record<string, string> {
  return Object.entries(t).reduce<Record<string, string>>((acc, [k, v]) => {
    const key = prefix ? `${prefix}.${k}` : k;
    return typeof v === "string" ? { ...acc, [key]: v } : { ...acc, ...flatten(v, key) };
  }, {});
}

const placeholders = (s: string) => (s.match(/{{\s*\w+\s*}}/g) ?? []).map((p) => p.replace(/\s/g, "")).sort();

describe("localisation", () => {
  const e = flatten(en as Tree);
  const a = flatten(ar as Tree);

  it("Arabic and English define exactly the same keys", () => {
    expect(Object.keys(a).sort()).toEqual(Object.keys(e).sort());
  });

  it("no translation is empty", () => {
    for (const [k, v] of [...Object.entries(e), ...Object.entries(a)]) expect(v.trim(), k).not.toBe("");
  });

  it("interpolation placeholders match between languages", () => {
    for (const k of Object.keys(e)) expect(placeholders(a[k] as string), k).toEqual(placeholders(e[k] as string));
  });

  it("Arabic strings are actually translated (contain Arabic script) unless they are brand or unit terms", () => {
    const allowed = new Set(["quality.h264", "quality.h265", "about.github", "settings.langEn", "info.tcp", "info.tlsws", "quality.auto_hint"]);
    const untranslated = Object.entries(a).filter(([k, v]) => !allowed.has(k) && !/[؀-ۿ]/.test(v));
    expect(untranslated.map(([k]) => k)).toEqual([]);
  });

  it("resolves system language and text direction", () => {
    expect(resolveLanguage("system", "ar-SA")).toBe("ar");
    expect(resolveLanguage("system", "en-US")).toBe("en");
    expect(resolveLanguage("en", "ar-SA")).toBe("en");
    expect(directionOf("ar")).toBe("rtl");
    expect(directionOf("en")).toBe("ltr");
  });

  it("switching language flips the document direction without a reload", () => {
    applyLanguage("ar");
    expect(document.documentElement.dir).toBe("rtl");
    applyLanguage("en");
    expect(document.documentElement.dir).toBe("ltr");
  });
});
