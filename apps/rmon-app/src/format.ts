import type { KillPolicy, Pressure, ProcState, SuspectKind } from "./types";
// Labels come from Rust (rmon-core) so the window, CLI and MCP say the same words.
import labels from "./bindings/labels.json" with { type: "json" };

const UNITS = ["Б", "КБ", "МБ", "ГБ", "ТБ"];

/** One decimal with Russian comma, ",0" dropped. Rounds like Rust: Math.round(v * 10). */
function num1(v: number): string {
  const t = Math.round(v * 10);
  const int = Math.trunc(t / 10);
  const frac = t % 10;
  return frac ? `${int},${frac}` : String(int);
}

/**
 * "3,1 ГБ", "412 МБ" — base 1024; unit steps up at 999,5 (so "1 ГБ", never "1000 МБ");
 * one decimal below 100, whole numbers from 100. Reference: bindings/format-cases.json.
 */
export function fmtBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return "—";
  const neg = bytes < 0;
  let v = Math.abs(bytes);
  let u = 0;
  while (v >= 999.5 && u < UNITS.length - 1) {
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
  const s = Math.max(0, Math.floor(now - startedAt));
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

export const PRESSURE_LABEL: Record<Pressure, string> = labels.pressure;
export const STATE_LABEL: Record<ProcState, string> = labels.proc_state;
export const KIND_LABEL: Record<SuspectKind, string> = labels.suspect_kind;
export const POLICY_LABEL: Record<KillPolicy["kind"], string> = labels.policy;

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

/** Process names that say nothing by themselves — the command line has to tell what they run. */
export const GENERIC_NAMES = new Set([
  "node", "claude", "bun", "deno", "python", "python3", "Python", "ruby", "java", "sh", "bash", "zsh",
  "npm", "npx", "pnpm", "uv", "uvx", "tsx",
]);

const PKG_RE = /node_modules\/(?:\.bin\/)?((?:@[^/\s]+\/)?[^/\s]+)/g;

/**
 * Short hint of what a generic process runs, from its command line:
 * "vite", "@playwright/mcp", "jest --watch", "uvicorn app:app". "" when nothing useful.
 */
export function cmdHint(name: string, cmd: string | string[]): string {
  const line = Array.isArray(cmd) ? cmd.join(" ") : cmd;
  const tokens = line.trim().split(/\s+/).filter(Boolean);
  // Drop argv[0] when it is the program itself ("node", "/opt/homebrew/bin/node").
  if (tokens.length && (tokens[0] === name || tokens[0].endsWith("/" + name))) tokens.shift();
  if (!tokens.length) return "";

  let label = "";
  let restFrom = -1;
  // A package inside node_modules is the most telling part: take the last one.
  for (let i = 0; i < tokens.length; i++) {
    PKG_RE.lastIndex = 0;
    let m: RegExpExecArray | null;
    let last: string | null = null;
    while ((m = PKG_RE.exec(tokens[i]))) last = m[1];
    if (last) {
      label = last;
      restFrom = i + 1;
    }
  }
  if (!label) {
    const i = tokens.findIndex((t) => !t.startsWith("-"));
    if (i < 0) return shorten(tokens.join(" "));
    // "-m uvicorn" style: skip flags, take the first word (basename of a path).
    label = tokens[i].slice(tokens[i].lastIndexOf("/") + 1);
    restFrom = i + 1;
  }
  const s = [label, ...tokens.slice(restFrom)].join(" ").trim();
  return s === name ? "" : shorten(s);
}

function shorten(s: string, max = 48): string {
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
}
