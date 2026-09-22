import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import ar from "./ar.json";
import en from "./en.json";

export type Lang = "en" | "ar";
export const languages: Lang[] = ["en", "ar"];

export function directionOf(lang: Lang): "ltr" | "rtl" {
  return lang === "ar" ? "rtl" : "ltr";
}

/** Resolves the `system` setting from the OS locale, defaulting to English. */
export function resolveLanguage(setting: string, osLocale: string): Lang {
  if (setting === "ar" || setting === "en") return setting;
  return osLocale.toLowerCase().startsWith("ar") ? "ar" : "en";
}

void i18n.use(initReactI18next).init({
  resources: { en: { translation: en }, ar: { translation: ar } },
  lng: "en",
  fallbackLng: "en",
  interpolation: { escapeValue: false },
  returnNull: false,
});

/** Applies language and text direction to the document so layout mirrors natively. */
export function applyLanguage(lang: Lang): void {
  void i18n.changeLanguage(lang);
  document.documentElement.lang = lang;
  document.documentElement.dir = directionOf(lang);
}

export default i18n;
