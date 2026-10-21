// Minimal i18n: FR and EN from day one. English is the fallback.
import en from "./en.json";
import fr from "./fr.json";

export type Locale = "en" | "fr";
export type Key = keyof typeof en;

const dictionaries: Record<Locale, Record<Key, string>> = { en, fr };

export function detectLocale(): Locale {
  return navigator.language.toLowerCase().startsWith("fr") ? "fr" : "en";
}

const current: Locale = detectLocale();

/** Translates `key` in the detected locale (or an explicit one). */
export function t(key: Key, locale: Locale = current): string {
  return dictionaries[locale][key] ?? dictionaries.en[key];
}

export const locale = current;
