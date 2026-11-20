// Fake memory guard + settings for `npm run dev` (part of the mock backend; never in Tauri).

import type { Backend } from "./api";
import type {
  AllowRule,
  Author,
  Dashboard,
  GuardEvent,
  GuardMode,
  GuardStatus,
  GuardZone,
  LegacyGuard,
  RuleSpec,
  Settings,
} from "./types";

const MB = 1024 * 1024;
const GB = 1024 * MB;
const MAX_EVENTS = 60;

type GuardApi = Pick<
  Backend,
  "guardStatus" | "onGuardEvent" | "retireLegacy" | "restoreLegacy" | "getSettings" | "saveSettings" | "openConfig" | "onOpenTab"
>;

const ALLOW: AllowRule[] = [
  { name: "tsgo", exe: "^tsgo$" },
  { name: "tsc", exe: "^node$", args: "/typescript/bin/tsc|\\btsc\\b", args_not: "--watch" },
  { name: "vitest", exe: "^node$", args: "vitest", args_not: "--watch|\\bwatch\\b" },
  { name: "jest", exe: "^node$", args: "jest" },
  { name: "node", exe: "^node$", args_not: "mcp|server|vite|next dev|--watch", parent_not: "\\.app/Contents/" },
  { name: "bun", exe: "^bun$", args_not: "dev|serve|--watch" },
];

const NEVER: RuleSpec[] = [
  { name: "kernel_task" },
  { name: "launchd" },
  { name: "WindowServer" },
  { name: "loginwindow" },
  { name: "Claude", path: "^/Applications/Claude\\.app/" },
  { name: "M Code Desktop", path: "^/Applications/M Code Desktop\\.app/" },
  { name: "browsers", path: "^/Applications/(Google Chrome|Safari|Arc)\\.app/" },
  { name: "Telegram", path: "^/Applications/Telegram\\.app/" },
  { name: "claude CLI", exe: "^claude$" },
  { name: "mcode-engine", exe: "^mcode-engine$" },
  { name: "rmon", exe: "^rmon(-app)?$" },
];

function author(project: string, title: string, cmd: string, sessionPid: number): Author {
  const cwd = `/Users/x/Programming - Main/${project}`;
  return {
    command: cmd,
    cwd,
    git_root: cwd,
    project,
    lane: null,
    session_cwd: cwd,
    origin: {
      kind: "claude_code",
      via: "claude",
      session_id: "5f0c2a7e-1d1b-4c55-9d1e-7b8a3c2f9e10",
      host_session_id: null,
      title,
      entrypoint: "cli",
      session_pid: sessionPid,
    },
    chain: [
      { pid: 39911, name: "tsgo", args: cmd },
      { pid: 39870, name: "node", args: "node node_modules/.bin/turbo run typecheck" },
      { pid: sessionPid, name: "claude", args: "claude --resume" },
      { pid: 3044, name: "zsh", args: "-zsh" },
      { pid: 2705, name: "M Code Desktop", args: "/Applications/M Code Desktop.app/Contents/MacOS/M Code Desktop" },
    ],
  };
}

export function createMockGuard(getDash: () => Dashboard): GuardApi & { tick(d: Dashboard): void } {
  const now0 = Math.floor(Date.now() / 1000);
  let settings: Settings = {
    app: { autostart: true, tray_title: true },
    guard: {
      enabled: true,
      dry_run: false,
      notify: true,
      notify_dry_run: false,
      sustain_secs: 20,
      after_kill_secs: 60,
      term_grace_secs: 5,
      min_victim_mb: 800,
      red: { pressure_level: 4, min_available_pct: 8, swap_red_mb: 14336, swap_red_from_level: 2 },
      limits: [
        { window_secs: 600, max: 3 },
        { window_secs: 3600, max: 6 },
      ],
    },
    thresholds: { hog_mb: 1024, hog_pct: 5, growing_mb_per_min: 15, idle_mb: 500, idle_age_min: 60, duplicate_count: 3 },
    config_file: "/Users/x/.config/rmon/config.toml",
  };
  let legacy: LegacyGuard = { kind: "running", pid: 812 };
  let zone: GuardZone | null = null;
  let zoneWhy = "";
  let redSince: number | null = null;
  let episodeReported = false;
  const listeners = new Set<(e: GuardEvent) => void>();
  const tabListeners = new Set<(t: "settings" | "guard" | "suspects") => void>();

  const tsgoCmd = "tsgo --build --watch=false -p tsconfig.json";
  const recent: GuardEvent[] = [
    {
      ev: "kill",
      ts: now0 - 1900,
      pressure: { level: 4, available_pct: 4, swap_used: 15.1 * GB },
      notice: { title: "Убит tsgo 4,6 ГБ", subtitle: "проект M Code · Claude Code «Старт»", body: "Красное 25 с, своп 15,1 ГБ" },
      outcome: "term",
      error: null,
      victim: { pid: 39911, started_at: now0 - 2300, kind: "tsgo", mem: 4.6 * GB, cmd: tsgoCmd },
      why: "давление красное 25 с, своп 15,1 ГБ",
      red_for_secs: 25,
      author: author("M Code", "Старт", tsgoCmd, 3051),
    },
    { ev: "zone", ts: now0 - 1925, pressure: { level: 4, available_pct: 5, swap_used: 14.8 * GB }, notice: null, zone: "red", why: "давление красное (4)" },
    {
      ev: "no-victim",
      ts: now0 - 5400,
      pressure: { level: 4, available_pct: 6, swap_used: 13.2 * GB },
      notice: null,
      why: "красное 31 с, но снимать некого",
      rejected: [
        { pid: 1708, name: "Google Chrome Helper (GPU)", mem: 1.1 * GB, why: "в списке «никогда»: browsers" },
        { pid: 3056, name: "claude", mem: 2.4 * GB, why: "в списке «никогда»: claude CLI" },
        { pid: 3930, name: "node", mem: 650 * MB, why: "не прогон: tsserver (правило node, args_not)" },
        { pid: 3884, name: "node", mem: 610 * MB, why: "меньше 800 МБ" },
      ],
    },
    {
      ev: "would-kill",
      ts: now0 - 9000,
      pressure: { level: 4, available_pct: 7, swap_used: 12.4 * GB },
      notice: null,
      victim: { pid: 4044, started_at: now0 - 9 * 3600, kind: "jest", mem: 1.6 * GB, cmd: "node node_modules/.bin/jest --watch" },
      why: "проба: красное 22 с",
      red_for_secs: 22,
      author: author("landing", "Тесты лендинга", "node node_modules/.bin/jest --watch", 3188),
    },
    { ev: "limit", ts: now0 - 12000, pressure: null, notice: null, why: "3 снятия за 10 мин — жду до 14:20" },
    { ev: "config-error", ts: now0 - 86000, pressure: null, notice: null, error: "config.toml, строка 14: min_victim_mb должно быть числом" },
    { ev: "start", ts: now0 - 86400, pressure: null, notice: null, mode: { kind: "armed" } },
  ];

  const mode = (): GuardMode => {
    if (!settings.guard.enabled) return { kind: "off" };
    if (legacy.kind === "running")
      return { kind: "standby", detail: `работает старый memory-guard (pid ${legacy.pid ?? "?"})` };
    return settings.guard.dry_run ? { kind: "dry_run" } : { kind: "armed" };
  };

  const emit = (e: GuardEvent) => {
    recent.unshift(e);
    if (recent.length > MAX_EVENTS) recent.length = MAX_EVENTS;
    for (const l of listeners) l(e);
  };
  const emitMode = () => emit({ ev: "mode", ts: Math.floor(Date.now() / 1000), pressure: null, notice: null, mode: mode() });

  function zoneOf(d: Dashboard): [GuardZone, string] {
    const s = d.system;
    const red = settings.guard.red;
    const level = s.pressure === "critical" ? 4 : s.pressure === "warning" ? 2 : 1;
    if (level >= red.pressure_level) return ["red", `давление ${level === 4 ? "красное" : "жёлтое"} (${level})`];
    if (s.available_pct <= red.min_available_pct) return ["red", `доступно ${s.available_pct} % ≤ ${red.min_available_pct} %`];
    if (level >= red.swap_red_from_level && s.swap_used >= red.swap_red_mb * MB)
      return ["red", `давление ${level}, своп ≥ ${Math.round(red.swap_red_mb / 1024)} ГБ`];
    if (level >= 2) return ["yellow", "давление жёлтое (2)"];
    return ["green", "давление в норме"];
  }

  const status = (): GuardStatus => {
    const now = Math.floor(Date.now() / 1000);
    const g = settings.guard;
    return {
      mode: mode(),
      zone,
      zone_why: zoneWhy,
      red_for_secs: redSince != null ? now - redSince : 0,
      legacy,
      config: { ...g, allow: ALLOW, never: NEVER },
      config_file: settings.config_file,
      recent: recent.slice(),
    };
  };

  const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

  return {
    tick(d) {
      if (!settings.guard.enabled) {
        zone = null;
        redSince = null;
        return;
      }
      const [z, why] = zoneOf(d);
      const now = d.taken_at;
      if (z !== zone) {
        zone = z;
        zoneWhy = why;
        redSince = z === "red" ? now : null;
        episodeReported = false;
        emit({ ev: "zone", ts: now, pressure: null, notice: null, zone: z, why });
      }
      if (z === "red" && redSince != null && !episodeReported && now - redSince >= settings.guard.sustain_secs) {
        episodeReported = true;
        emit({
          ev: "no-victim",
          ts: now,
          pressure: { level: 4, available_pct: d.system.available_pct, swap_used: d.system.swap_used },
          notice: null,
          why: `красное ${now - redSince} с, но снимать некого`,
          rejected: [
            { pid: 3056, name: "claude", mem: 2.4 * GB, why: "в списке «никогда»: claude CLI" },
            { pid: 5272, name: "ollama", mem: 1.4 * GB, why: "не прогон: нет правила «можно»" },
          ],
        });
      }
    },
    guardStatus: async () => {
      await delay(60);
      void getDash;
      return status();
    },
    onGuardEvent: async (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    retireLegacy: async () => {
      await delay(500);
      if (legacy.kind !== "running" && legacy.kind !== "installed") throw "Старый сторож не установлен — переносить нечего";
      legacy = { kind: "retired" };
      emitMode();
      return status();
    },
    restoreLegacy: async () => {
      await delay(500);
      if (legacy.kind !== "retired") throw "Старый сторож не был перенесён — возвращать нечего";
      legacy = { kind: "running", pid: 9120 };
      emitMode();
      return status();
    },
    getSettings: async () => {
      await delay(60);
      return structuredClone(settings);
    },
    saveSettings: async (next) => {
      await delay(250);
      const g = next.guard;
      if (g.min_victim_mb < 100) throw "Жертва от: меньше 100 МБ нельзя — сторож начнёт снимать мелочь";
      if (g.sustain_secs < 5) throw "«Держится» — не меньше 5 с, иначе сторож сработает на случайный всплеск";
      if (g.red.min_available_pct > 50) throw "«Доступно не больше» — не больше 50 %";
      for (const l of g.limits)
        if (l.max < 1 || l.window_secs < 10) throw "Предел: не меньше 1 снятия и окно не короче 10 с";
      if (next.thresholds.duplicate_count < 2) throw "Брошенных копий — от 2";
      const modeBefore = JSON.stringify(mode());
      settings = { ...structuredClone(next), config_file: settings.config_file };
      if (JSON.stringify(mode()) !== modeBefore) emitMode();
      return structuredClone(settings);
    },
    openConfig: async () => {
      await delay(50);
      console.info(`[mock] open_config: ${settings.config_file}`);
    },
    onOpenTab: async (cb) => {
      tabListeners.add(cb);
      return () => tabListeners.delete(cb);
    },
  };
}

// Dev only: see mock.ts — reload instead of running two fake guards.
if (import.meta.hot) import.meta.hot.accept(() => location.reload());
