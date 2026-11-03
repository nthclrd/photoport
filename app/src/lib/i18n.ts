import fr from "../locales/fr.json";
import en from "../locales/en.json";

type Dict = Record<string, string>;

export const currentLang: "fr" | "en" = (navigator.language || "fr").toLowerCase().startsWith("en") ? "en" : "fr";
const dict: Dict = currentLang === "en" ? en : fr;

/** `{name}` inserts a value; `{n|one|many}` picks a form from the number n. */
export function t(key: string, params: Record<string, string | number> = {}): string {
  const raw = dict[key] ?? (fr as Dict)[key] ?? key;
  return raw.replace(/\{(\w+)(?:\|([^|}]*)\|([^}]*))?\}/g, (whole, k, one, many) => {
    const v = params[k];
    if (v === undefined) return whole;
    if (one !== undefined) return Number(v) <= 1 && currentLang === "fr" ? one : Number(v) === 1 ? one : many;
    return String(v);
  });
}
