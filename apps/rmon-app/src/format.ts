import type { KillPolicy, Pressure, ProcState, SuspectKind } from "./types";

const UNITS = ["Б", "КБ", "МБ", "ГБ", "ТБ"];

/** Number with Russian decimal comma, one decimal, ",0" dropped. */
function num1(v: number): string {
  const s = v.toFixed(1);
  return (s.endsWith(".0") ? s.slice(0, -2) : s).replace(".", ",");
}

/** "3,1 ГБ", "412 МБ" — base 1024, one decimal below 100, integers from 100 up. */
export function fmtBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return "—";
  const neg = bytes < 0;
  let v = Math.abs(bytes);
  let u = 0;
  // Switch unit at 1000 so we print "0,98 ГБ" → "1 ГБ" instead of "1003 МБ".
  while (v >= 1000 && u < UNITS.length - 1) {
    v /= 1024;
    u++;
  }
  const body = u === 0 || v >= 100 ? String(Math.round(v)) : num1(v);
  return (neg ? "−" : "") + body + " " + UNITS[u];
}

const MB = 1024 * 1024;

/** Growth in MB/min with sign: "+40,2", "−3", "0". null → "—". */
export function fmtGrowth(bytesPerMin: number | null): string {
  if (bytesPerMin == null) return "—";
  const mb = bytesPerMin / MB;
  if (Math.abs(mb) < 0.05) return "0";
  const body = Math.abs(mb) >= 100 ? String(Math.round(Math.abs(mb))) : num1(Math.abs(mb));
  return (mb > 0 ? "+" : "−") + body;
}

export function fmtPct(v: number): string {
  if (v < 0.05) return "0";
  return v >= 100 ? String(Math.round(v)) : num1(v);
}

/** Age from unix seconds: "45 с", "12 мин", "3 ч 5 мин", "4 д 2 ч". */
export function fmtAge(startedAt: number, now: number): string {
  let s = Math.max(0, Math.floor(now - startedAt));
  if (s < 60) return `${s} с`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} мин`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h} ч ${m % 60} мин` : `${h} ч`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d} д ${h % 24} ч` : `${d} д`;
}

const pad = (n: number) => (n < 10 ? "0" + n : String(n));

export function fmtClock(ts: number): string {
  const d = new Date(ts * 1000);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

export function fmtDateTime(ts: number): string {
  const d = new Date(ts * 1000);
  return `${pad(d.getDate())}.${pad(d.getMonth() + 1)} ${fmtClock(ts)}`;
}

export const PRESSURE_LABEL: Record<Pressure, string> = {
  normal: "нормальное",
  warning: "повышенное",
  critical: "критическое",
};

export const STATE_LABEL: Record<ProcState, string> = {
  running: "работает",
  sleeping: "спит",
  stopped: "остановлен",
  zombie: "зомби",
  unknown: "неизвестно",
};

export const KIND_LABEL: Record<SuspectKind, string> = {
  hog: "Большой",
  growing: "Растёт",
  orphan: "Сирота",
  idle: "Простаивает",
  duplicate: "Дубль",
  zombie: "Зомби",
};

export const POLICY_LABEL: Record<KillPolicy["kind"], string> = {
  protected: "защищён",
  confirm: "с подтверждением",
  free: "можно снять",
};

export function policyReason(p: KillPolicy): string {
  return p.kind === "free" ? "" : p.reason;
}

/** Errors from Tauri invoke arrive as plain strings; normalize anything else. */
export function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

/** Russian plural: plural(5, "процесс", "процесса", "процессов") → "процессов". */
export function plural(n: number, one: string, few: string, many: string): string {
  const m10 = n % 10;
  const m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
}

/** Reason as its own sentence: capitalized, trailing period dropped (the caller adds one). */
export function clause(s: string): string {
  const c = s.replace(/[.\s]+$/, "");
  return c.charAt(0).toUpperCase() + c.slice(1);
}

/**
 * Short hint of what a generic process runs: "vite", "@playwright/mcp", "jest --watch".
 * Returns "" when the args add nothing beyond the name.
 */
export function cmdHint(name: string, args: string[]): string {
  const rest = args.slice(1);
  const i = rest.findIndex((a) => !a.startsWith("-"));
  if (i < 0) return rest.join(" ").slice(0, 48);
  const main = rest[i];
  let label: string;
  const nm = main.lastIndexOf("node_modules/");
  if (nm >= 0) {
    const parts = main.slice(nm + 13).split("/");
    label = parts[0] === ".bin" ? parts[1] ?? "" : parts[0].startsWith("@") ? `${parts[0]}/${parts[1] ?? ""}` : parts[0];
  } else {
    label = main.slice(main.lastIndexOf("/") + 1);
  }
  const s = [label, ...rest.slice(i + 1)].join(" ").trim();
  return s === name ? "" : s.length > 48 ? s.slice(0, 47) + "…" : s;
}
