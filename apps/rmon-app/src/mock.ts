// Fake backend for `npm run dev` in a plain browser. Never bundled into the main chunk:
// api.ts imports it dynamically only when not running inside Tauri.

import type { Backend } from "./api";
import { fmtAge, fmtBytes, fmtGrowth } from "./format";
import type {
  AppGroup,
  HistoryPoint,
  JournalEntry,
  KillArgs,
  KillOutcome,
  KillPolicy,
  Pressure,
  ProcessRow,
  ProcBrief,
  ProcessDetails,
  ProcState,
  Dashboard,
  Suspect,
  SystemMemory,
} from "./types";

const MB = 1024 * 1024;
const GB = 1024 * MB;
const TOTAL = 24 * GB;
const SWAP_TOTAL = 18 * GB;
const TICK_S = 2;
const HISTORY_S = 15 * 60;
const USER_UID = 501;

// Deterministic RNG so the fake machine looks the same on every reload.
function mulberry32(seed: number) {
  let a = seed;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rnd = mulberry32(20261120);
const between = (lo: number, hi: number) => lo + rnd() * (hi - lo);
const pick = <T,>(xs: T[]): T => xs[Math.floor(rnd() * xs.length)];

interface MockProc {
  pid: number;
  ppid: number;
  name: string;
  exe: string | null;
  args: string[];
  uid: number;
  mem: number; // footprint (or rss when footprint is hidden)
  hideFootprint: boolean;
  rate: number; // bytes/min drift
  cpuBase: number;
  threads: number;
  state: ProcState;
  started_at: number;
  app_group: string;
  policy: KillPolicy;
  cwd: string | null;
}

const now0 = Math.floor(Date.now() / 1000);
let nextPid = 300;
const procs: MockProc[] = [];

const PROTECTED_SYS: KillPolicy = { kind: "protected", reason: "системный процесс macOS (/System, /usr)" };
const FREE: KillPolicy = { kind: "free" };
const confirm = (reason: string): KillPolicy => ({ kind: "confirm", reason });

function add(p: Partial<MockProc> & Pick<MockProc, "name" | "mem">): MockProc {
  const proc: MockProc = {
    pid: p.pid ?? (nextPid += Math.floor(between(1, 40))),
    ppid: p.ppid ?? 1,
    exe: null,
    args: [p.name],
    uid: USER_UID,
    hideFootprint: false,
    rate: 0,
    cpuBase: 0.2,
    threads: Math.floor(between(3, 30)),
    state: "sleeping",
    started_at: now0 - Math.floor(between(600, 3 * 86400)),
    app_group: p.name,
    policy: FREE,
    cwd: (p.uid ?? USER_UID) === USER_UID ? "/Users/x" : "/",
    ...p,
  };
  procs.push(proc);
  return proc;
}

function buildMachine() {
  // --- Kernel & core system (protected, root → footprint hidden) ---
  add({ pid: 0, ppid: 0, name: "kernel_task", mem: 2.1 * GB, uid: 0, hideFootprint: true, threads: 540, cpuBase: 4,
    policy: { kind: "protected", reason: "ядро macOS" }, started_at: now0 - 5 * 86400 });
  add({ pid: 1, ppid: 0, name: "launchd", exe: "/sbin/launchd", mem: 14 * MB, uid: 0, hideFootprint: true, threads: 6,
    policy: { kind: "protected", reason: "launchd — корень всех процессов" }, started_at: now0 - 5 * 86400 });
  add({ pid: 152, name: "WindowServer", exe: "/System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer",
    mem: 1.3 * GB, uid: 88, hideFootprint: true, threads: 22, cpuBase: 9, state: "running",
    policy: { kind: "protected", reason: "WindowServer — без него гаснет экран и закрываются все окна" },
    started_at: now0 - 5 * 86400 });
  add({ pid: 389, name: "loginwindow", exe: "/System/Library/CoreServices/loginwindow.app/Contents/MacOS/loginwindow",
    mem: 48 * MB, policy: { kind: "protected", reason: "loginwindow — снятие завершит сеанс" }, started_at: now0 - 5 * 86400 });

  const daemons = [
    "mds", "mds_stores", "mdworker_shared", "opendirectoryd", "configd", "coreaudiod", "bluetoothd", "airportd",
    "logd", "notifyd", "distnoted", "powerd", "syspolicyd", "trustd", "securityd", "runningboardd", "cfprefsd",
    "fseventsd", "diskarbitrationd", "coreduetd", "symptomsd", "locationd", "sandboxd", "kernelmanagerd", "thermalmonitord",
    "usbd", "watchdogd", "timed", "apsd", "nsurlsessiond", "containermanagerd", "backupd", "softwareupdated", "corebrightnessd",
  ];
  for (const d of daemons) {
    add({ name: d, exe: `/usr/libexec/${d}`, args: [`/usr/libexec/${d}`], uid: 0, hideFootprint: true,
      mem: between(3, d.startsWith("mds") ? 380 : 60) * MB, policy: PROTECTED_SYS, started_at: now0 - 5 * 86400 });
  }
  const userAgents = [
    "Finder", "Dock", "SystemUIServer", "ControlCenter", "NotificationCenter", "Spotlight", "WallpaperAgent",
    "photoanalysisd", "suggestd", "cloudd", "bird", "sharingd", "CommCenter", "UserEventAgent", "TextInputMenuAgent",
    "universalaccessd", "AirPlayUIAgent", "iconservicesagent", "QuickLookUIService", "familycircled", "knowledge-agent",
    "duetexpertd", "routined", "assistantd", "siriknowledged", "callservicesd", "ViewBridgeAuxiliary", "CoreLocationAgent",
    "akd", "secd", "keyboardservicesd", "chronod", "mediaanalysisd", "corespotlightd", "parsecd", "contactsd", "calaccessd",
  ];
  for (const a of userAgents) {
    add({ name: a, exe: `/System/Library/CoreServices/${a}`, args: [`/System/Library/CoreServices/${a}`],
      mem: between(8, a === "Finder" || a === "mediaanalysisd" ? 420 : 120) * MB, policy: PROTECTED_SYS,
      started_at: now0 - Math.floor(between(3600, 5 * 86400)) });
  }

  // --- Browser ---
  const chrome = add({ name: "Google Chrome", exe: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    mem: 680 * MB, threads: 48, cpuBase: 3, app_group: "Google Chrome",
    policy: confirm("браузер владельца: закроются все вкладки"), started_at: now0 - 2 * 86400 });
  add({ name: "Google Chrome Helper (GPU)", ppid: chrome.pid, mem: 1.1 * GB, threads: 20, cpuBase: 5,
    app_group: "Google Chrome", policy: confirm("хелпер браузера: может закрыть вкладки"), started_at: chrome.started_at });
  for (let i = 0; i < 46; i++) {
    const renderer = rnd() < 0.8;
    add({ name: renderer ? "Google Chrome Helper (Renderer)" : "Google Chrome Helper",
      args: ["Google Chrome Helper", renderer ? "--type=renderer" : "--type=utility", `--renderer-client-id=${i + 7}`],
      ppid: chrome.pid, mem: (renderer ? between(60, 640) : between(20, 90)) * MB,
      rate: i === 3 ? 22 * MB : 0, cpuBase: renderer ? between(0, 3) : 0.1, app_group: "Google Chrome",
      policy: confirm("вкладка браузера: несохранённое в ней пропадёт"),
      started_at: now0 - Math.floor(between(300, 2 * 86400)) });
  }

  // --- Owner's apps ---
  const claudeApp = add({ name: "Claude", exe: "/Applications/Claude.app/Contents/MacOS/Claude", mem: 420 * MB,
    threads: 40, cpuBase: 1.5, app_group: "Claude", policy: confirm("приложение Claude: открытый разговор прервётся") });
  for (let i = 0; i < 6; i++)
    add({ name: i === 0 ? "Claude Helper (GPU)" : "Claude Helper (Renderer)", ppid: claudeApp.pid,
      mem: between(80, 520) * MB, app_group: "Claude", policy: confirm("хелпер Claude"), started_at: claudeApp.started_at });

  const mcode = add({ name: "M Code Desktop", exe: "/Applications/M Code Desktop.app/Contents/MacOS/M Code Desktop",
    mem: 610 * MB, threads: 52, cpuBase: 2, app_group: "M Code Desktop", policy: confirm("M Code Desktop: оборвутся сессии агентов") });
  for (let i = 0; i < 5; i++)
    add({ name: "M Code Desktop Helper", ppid: mcode.pid, mem: between(120, 700) * MB, app_group: "M Code Desktop",
      policy: confirm("хелпер M Code Desktop"), started_at: mcode.started_at });
  add({ name: "mcode-engine", ppid: mcode.pid, mem: 940 * MB, rate: 6 * MB, cpuBase: 4, app_group: "M Code Desktop",
    args: ["mcode-engine", "--port", "47110"], policy: confirm("mcode-engine: оборвутся все сессии M Code") });

  add({ name: "Telegram", exe: "/Applications/Telegram.app/Contents/MacOS/Telegram", mem: 390 * MB, app_group: "Telegram",
    policy: confirm("Telegram: приложение владельца") });
  const code = add({ name: "Code", exe: "/Applications/Visual Studio Code.app/Contents/MacOS/Electron", mem: 310 * MB,
    app_group: "Visual Studio Code", policy: confirm("редактор: несохранённые файлы пропадут") });
  for (let i = 0; i < 7; i++)
    add({ name: i < 2 ? "Code Helper (Renderer)" : "Code Helper (Plugin)", ppid: code.pid, mem: between(60, 480) * MB,
      app_group: "Visual Studio Code", policy: confirm("хелпер редактора"), started_at: code.started_at });

  // --- claude CLI sessions ---
  const claudeSessions: MockProc[] = [];
  const sessionProjects = ["Resource Management", "ultra-web", "chatplace-admin", "M Code", "landing", "Hooks", "docs-site", "Resource Management"];
  for (let i = 0; i < 8; i++) {
    const cwd = `/Users/x/Programming - Main/${sessionProjects[i]}`;
    const started = now0 - Math.floor(between(900, 40000));
    const shell = add({ name: "zsh", args: ["-zsh"], mem: between(2, 6) * MB, ppid: i < 5 ? mcode.pid : code.pid, cwd,
      started_at: started - 5, app_group: i < 5 ? "M Code Desktop" : "Visual Studio Code",
      policy: confirm("оболочка терминала: закроется вкладка") });
    claudeSessions.push(add({ name: "claude", exe: "/Users/x/.local/bin/claude", cwd,
      args: ["claude", ...(i % 3 === 0 ? ["--resume"] : []), ...(i === 5 ? ["--dangerously-skip-permissions"] : [])],
      mem: (i === 2 ? 2.4 * GB : between(280, 900) * MB), rate: i === 2 ? 38 * MB : between(-1, 3) * MB,
      threads: 14, cpuBase: between(0.5, 6), app_group: "Claude Code", ppid: shell.pid,
      policy: confirm("сессия claude CLI: разговор оборвётся"), started_at: started }));
  }

  // --- node zoo ---
  const mcpServers = [
    "@modelcontextprotocol/server-filesystem", "@playwright/mcp", "mcp-server-github", "@upstash/context7-mcp",
  ];
  for (const s of claudeSessions) {
    for (const m of mcpServers) {
      if (rnd() < 0.55) continue;
      add({ name: "node", exe: "/opt/homebrew/bin/node", args: ["node", `/Users/x/.npm/_npx/4f1c/node_modules/${m}/dist/index.js`],
        mem: between(40, 140) * MB, ppid: s.pid, app_group: "node", cpuBase: 0.05, started_at: s.started_at + 3, cwd: s.cwd });
    }
  }
  // Six identical stale MCP servers from dead sessions → Duplicate + Orphan.
  for (let i = 0; i < 6; i++)
    add({ name: "node", exe: "/opt/homebrew/bin/node", args: ["node", "/Users/x/.npm/_npx/9a2e/node_modules/mcp-memory-bank/dist/index.js"],
      mem: between(70, 110) * MB, ppid: 1, cpuBase: 0, started_at: now0 - Math.floor(between(20000, 90000)),
      cwd: `/Users/x/Programming - Main/${pick(sessionProjects)}` });

  const projects = ["rmon-app", "landing", "ultra-web", "chatplace-admin", "mcode-ui", "docs-site"];
  for (const p of projects) {
    add({ name: "node", exe: "/opt/homebrew/bin/node", args: ["node", `/Users/x/Programming - Main/${p}/node_modules/.bin/vite`],
      mem: between(120, 420) * MB, cpuBase: 0.3, ppid: rnd() < 0.3 ? 1 : code.pid, cwd: `/Users/x/Programming - Main/${p}` });
    add({ name: "node", exe: "/opt/homebrew/bin/node",
      args: ["node", "/Applications/Visual Studio Code.app/Contents/Resources/app/extensions/node_modules/typescript/lib/tsserver.js", "--project", p],
      mem: between(300, 1300) * MB, rate: p === "ultra-web" ? 18 * MB : 0, cpuBase: 0.5, ppid: code.pid, cwd: `/Users/x/Programming - Main/${p}` });
  }
  // A forgotten 1.6 GB jest run with no CPU → Idle + Orphan.
  add({ name: "node", exe: "/opt/homebrew/bin/node", args: ["node", "/Users/x/Programming - Main/landing/node_modules/.bin/jest", "--watch"],
    mem: 1.6 * GB, cpuBase: 0, ppid: 1, started_at: now0 - 9 * 3600, cwd: "/Users/x/Programming - Main/landing" });
  for (let i = 0; i < 40; i++)
    add({ name: "node", exe: "/opt/homebrew/bin/node",
      args: ["node", pick(["eslint_d", "prettierd", "postcss-loader", "next-server", "esbuild --service", "turbo daemon", "pnpm install"])],
      mem: between(30, 260) * MB, cpuBase: between(0, 1.5), ppid: pick([code.pid, 1, claudeSessions[0].pid]) });

  // --- other dev tools ---
  add({ name: "tsgo", exe: "/Users/x/Programming - Main/ultra-web/node_modules/.bin/tsgo", args: ["tsgo", "--build", "--watch"],
    mem: 3.4 * GB, rate: 55 * MB, cpuBase: 85, state: "running", threads: 18, started_at: now0 - 420,
    cwd: "/Users/x/Programming - Main/ultra-web", ppid: claudeSessions[1].pid });
  add({ name: "tsgo", exe: "/Users/x/Programming - Main/landing/node_modules/.bin/tsgo", args: ["tsgo", "-p", "."],
    mem: 720 * MB, cpuBase: 40, state: "running", started_at: now0 - 95 });
  for (let i = 0; i < 6; i++)
    add({ name: "bun", exe: "/Users/x/.bun/bin/bun", args: ["bun", pick(["run dev", "test --watch", "x mcp-proxy"])],
      mem: between(60, 480) * MB, cpuBase: between(0, 2), ppid: i === 0 ? 1 : code.pid });
  for (let i = 0; i < 4; i++)
    add({ name: "python3", exe: "/opt/homebrew/bin/python3", args: ["python3", pick(["-m http.server", "train.py", "notebook.py", "-m uvicorn app:app"])],
      mem: between(40, 900) * MB, ppid: i === 3 ? 1 : code.pid });
  add({ name: "rust-analyzer", args: ["rust-analyzer"], mem: 1.2 * GB, cpuBase: 1, ppid: code.pid });
  add({ name: "cargo", args: ["cargo", "build", "--release", "-p", "rmon-app"], mem: 90 * MB, cpuBase: 5, ppid: code.pid, started_at: now0 - 60 });
  add({ name: "rustc", args: ["rustc", "--crate-name", "rmon_core", "--edition=2024"], mem: 640 * MB, cpuBase: 98, state: "running", started_at: now0 - 40 });
  add({ name: "node", args: ["node"], mem: 0, state: "zombie", threads: 0, cpuBase: 0, ppid: claudeSessions[1].pid });
  add({ name: "rmon-app", exe: "/Applications/Resource Monitor.app/Contents/MacOS/rmon-app", mem: 58 * MB,
    app_group: "Resource Monitor", policy: { kind: "protected", reason: "это сам rmon" } });

  // Filler so we land near 300 processes.
  add({ name: "ollama", args: ["ollama", "serve"], mem: 1.4 * GB, cpuBase: 0.1, ppid: 1 });
  const fillers = ["ssh-agent", "gpg-agent", "zsh", "tmux", "less", "git", "rg", "fzf", "caffeinate", "docker-proxy"];
  while (procs.length < 300) {
    const n = pick(fillers);
    add({ name: n, args: [n], mem: between(1, 30) * MB, ppid: code.pid });
  }
}
buildMachine();

// ---------------- state synthesis ----------------

const history: HistoryPoint[] = [];
let counters = { pageins: 81_234_112, pageouts: 4_120_771, swapins: 2_315_887, swapouts: 3_004_118, compressions: 391_200_771, decompressions: 377_812_004 };

function systemAt(t: number): Omit<SystemMemory, "counters"> {
  // Slow waves + noise; pressure comes out of how full RAM is.
  const wave = Math.sin(t / 170) * 0.5 + Math.sin(t / 47) * 0.2;
  const app = 11.2 * GB + wave * 1.6 * GB + between(-60, 60) * MB;
  const wired = 3.1 * GB + between(-20, 20) * MB;
  const compressed = 6.8 * GB + Math.sin(t / 230) * 1.4 * GB;
  const used = app + wired + compressed;
  const free = Math.max(120 * MB, 0.45 * GB - wave * 0.3 * GB);
  const cached_files = Math.max(0, TOTAL - used - free);
  const swap_used = 9.2 * GB + Math.sin(t / 300) * 1.8 * GB + Math.max(0, wave) * 0.8 * GB;
  const level = (used / TOTAL) * 0.6 + wave * 0.4;
  const pressure: Pressure = level > 0.72 ? "critical" : level > 0.52 ? "warning" : "normal";
  const available_pct = Math.round(Math.max(3, Math.min(60, 40 - wave * 35)));
  return { total: TOTAL, used, app, wired, compressed, cached_files, free, swap_total: SWAP_TOTAL, swap_used, pressure, available_pct };
}

for (let t = now0 - HISTORY_S; t < now0; t += TICK_S) {
  const s = systemAt(t);
  history.push({ t, used: s.used, swap_used: s.swap_used, compressed: s.compressed, pressure: s.pressure });
}

function tickProcs(now: number) {
  for (const p of procs) {
    if (p.state === "zombie") continue;
    p.mem = Math.max(2 * MB, p.mem + (p.rate * TICK_S) / 60 + between(-0.4, 0.4) * MB * (p.mem > GB ? 4 : 1));
  }
  // Process churn: short-lived node/esbuild children come and go.
  if (rnd() < 0.35) {
    add({ name: pick(["node", "esbuild", "git", "rg"]), args: ["node", "-e", "child"], mem: between(20, 160) * MB,
      cpuBase: between(5, 60), state: "running", started_at: now, ppid: procs[procs.length - 1].pid });
  }
  if (procs.length > 305) {
    const i = procs.findIndex((p) => p.started_at > now0 && p.started_at < now - 10);
    if (i >= 0) procs.splice(i, 1);
  }
}

function toRow(p: MockProc, now: number): ProcessRow {
  const age = now - p.started_at;
  const cpu = p.state === "zombie" ? 0 : Math.max(0, p.cpuBase + between(-0.5, 0.5) * (p.cpuBase + 0.3));
  return {
    pid: p.pid,
    ppid: p.ppid,
    name: p.name,
    exe: p.exe,
    args: p.args,
    uid: p.uid,
    footprint: p.hideFootprint ? null : Math.round(p.mem),
    rss: Math.round(p.hideFootprint ? p.mem : p.mem * 0.82),
    cpu_pct: Math.round(cpu * 10) / 10,
    threads: p.threads,
    state: p.state,
    started_at: p.started_at,
    app_group: p.app_group,
    growth_per_min: age < 60 || p.state === "zombie" ? null : p.rate + (p.rate ? between(-0.1, 0.1) * p.rate : between(-0.04, 0.04) * MB),
    policy: p.policy,
  };
}

const DEV = new Set(["node", "bun", "python3", "tsgo", "claude", "cargo", "rustc", "esbuild"]);
const cmdOf = (r: { args: string[]; exe: string | null; name: string }) =>
  (r.args.length ? r.args.join(" ") : r.exe ?? r.name).slice(0, 200);

function analyze(rows: ProcessRow[], now: number): Suspect[] {
  const out: Suspect[] = [];
  // Abandoned copies of one command (orphans with the same command line) share a group key.
  const orphanCount = new Map<string, number>();
  for (const r of rows) if (r.ppid === 1 && DEV.has(r.name)) orphanCount.set(cmdOf(r), (orphanCount.get(cmdOf(r)) ?? 0) + 1);
  const base = (r: ProcessRow) => {
    const cmd = cmdOf(r);
    return { pid: r.pid, name: r.name, app_group: r.app_group, started_at: r.started_at, policy: r.policy, cmd,
      group: r.ppid === 1 && (orphanCount.get(cmd) ?? 0) >= 2 ? `copies:${cmd}` : null };
  };
  const dupes = new Map<string, ProcessRow[]>();
  for (const r of rows) {
    const mem = r.footprint ?? r.rss;
    if (r.state === "zombie") {
      out.push({ ...base(r), kind: "zombie", score: 30, reclaim: 0, reason: "зомби: процесс завершился, а родитель не забрал код выхода" });
      continue;
    }
    if (r.footprint != null && r.footprint >= GB) {
      out.push({ ...base(r), kind: "hog", score: Math.min(95, Math.round(45 + (r.footprint / GB) * 12)), reclaim: r.footprint,
        reason: `${r.name} занимает ${fmtBytes(r.footprint)} — ${Math.round((r.footprint / TOTAL) * 100)} % ОЗУ` });
    }
    if (r.growth_per_min != null && r.growth_per_min >= 15 * MB) {
      out.push({ ...base(r), kind: "growing", score: Math.min(98, Math.round(55 + r.growth_per_min / MB / 2)), reclaim: mem,
        reason: `${r.name} ${fmtBytes(mem)}, растёт ${fmtGrowth(r.growth_per_min)} МБ/мин ${Math.min(15, Math.floor((now - r.started_at) / 60))} мин подряд` });
    }
    if (r.ppid === 1 && DEV.has(r.name)) {
      out.push({ ...base(r), kind: "orphan", score: 48, reclaim: mem,
        reason: `родитель завершился, а ${r.name} остался (ppid 1) уже ${fmtAge(r.started_at, now)}` });
    }
    if (mem >= 500 * MB && r.cpu_pct < 0.5 && now - r.started_at > 3600 && r.policy.kind === "free") {
      out.push({ ...base(r), kind: "idle", score: 42, reclaim: mem,
        reason: `${fmtBytes(mem)} и почти без CPU уже ${fmtAge(r.started_at, now)} — похоже на забытый процесс` });
    }
    if (r.args.some((a) => a.includes("_npx"))) {
      const key = r.args.join(" ");
      const list = dupes.get(key);
      if (list) list.push(r);
      else dupes.set(key, [r]);
    }
  }
  for (const list of dupes.values()) {
    if (list.length < 3) continue;
    const sorted = [...list].sort((a, b) => b.started_at - a.started_at);
    for (const r of sorted.slice(1)) {
      out.push({ ...base(r), kind: "duplicate", score: 35, reclaim: r.footprint ?? r.rss,
        reason: `${list.length} одинаковых запусков: ${r.args.slice(1).join(" ").slice(0, 80)}` });
    }
  }
  return out.sort((a, b) => b.score - a.score);
}

function groupsOf(rows: ProcessRow[]): AppGroup[] {
  const m = new Map<string, AppGroup>();
  for (const r of rows) {
    let g = m.get(r.app_group);
    if (!g) m.set(r.app_group, (g = { name: r.app_group, pids: [], footprint: 0, count: 0 }));
    g.pids.push(r.pid);
    g.footprint += r.footprint ?? r.rss;
    g.count++;
  }
  return [...m.values()].sort((a, b) => b.footprint - a.footprint);
}

function makeDashboard(now: number): Dashboard {
  const s = systemAt(now);
  counters = {
    pageins: counters.pageins + Math.floor(between(50, 900)),
    pageouts: counters.pageouts + Math.floor(between(0, 120)),
    swapins: counters.swapins + Math.floor(between(0, 300)),
    swapouts: counters.swapouts + Math.floor(between(0, 260)),
    compressions: counters.compressions + Math.floor(between(1000, 9000)),
    decompressions: counters.decompressions + Math.floor(between(900, 8000)),
  };
  history.push({ t: now, used: s.used, swap_used: s.swap_used, compressed: s.compressed, pressure: s.pressure });
  while (history.length && history[0].t < now - HISTORY_S) history.shift();
  const processes = procs.map((p) => toRow(p, now));
  return {
    taken_at: now,
    system: { ...s, counters: { ...counters } },
    history: history.slice(),
    groups: groupsOf(processes),
    processes,
    suspects: analyze(processes, now),
  };
}

let current: Dashboard = makeDashboard(now0);

const journal: JournalEntry[] = [
  { source: "rmon", ts: now0 - 640, text: "app: снят node (48120) SIGTERM, освобождено 1,2 ГБ — «зависший vite»", raw: { requested_by: "app", pid: 48120 } },
  { source: "memory-guard", ts: now0 - 1900, text: "красная зона: снят tsgo (39911) 4,6 ГБ, своп 15,1 ГБ", raw: { action: "kill", pid: 39911, rss_mb: 4710 } },
  { source: "rmon", ts: now0 - 3600, text: "mcp: отказ — WindowServer защищён", raw: { requested_by: "mcp", pid: 152, refused: true } },
  { source: "memory-guard", ts: now0 - 7200, text: "жёлтая зона: давление повышенное, своп 11,4 ГБ — без действий", raw: { level: "warning" } },
  { source: "memory-guard", ts: null, text: "сторож запущен (интервал 10 с)", raw: { event: "start" } },
];

const listeners = new Set<(d: Dashboard) => void>();
let timer: ReturnType<typeof setInterval> | null = null;

function publish() {
  for (const l of listeners) l(current);
}

function ensureTimer() {
  if (timer) return;
  timer = setInterval(() => {
    const now = Math.floor(Date.now() / 1000);
    tickProcs(now);
    current = makeDashboard(now);
    publish();
  }, TICK_S * 1000);
}

const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

const brief = (p: MockProc): ProcBrief => ({
  pid: p.pid,
  name: p.name,
  started_at: p.started_at,
  reclaim: Math.round(p.mem),
  cmd: cmdOf(p).slice(0, 120),
});

async function details(pid: number, startedAt: number): Promise<ProcessDetails> {
  await delay(120);
  const byPid = new Map(procs.map((p) => [p.pid, p]));
  const p = byPid.get(pid);
  if (!p || p.started_at !== startedAt) throw `процесса ${pid} уже нет`;
  const now = Math.floor(Date.now() / 1000);
  const row = current.processes.find((r) => r.pid === pid) ?? toRow(p, now);

  const ancestors: ProcBrief[] = [];
  for (let q = byPid.get(p.ppid); q && ancestors.length < 20; q = q.pid === q.ppid ? undefined : byPid.get(q.ppid)) {
    ancestors.push(brief(q));
    if (q.pid <= 1) break;
  }
  const children = new Map<number, MockProc[]>();
  for (const q of procs) {
    const list = children.get(q.ppid);
    if (list) list.push(q);
    else children.set(q.ppid, [q]);
  }
  const descendants: ProcBrief[] = [];
  const queue = pid > 1 ? [pid] : [];
  while (queue.length && descendants.length < 200) {
    for (const c of children.get(queue.shift()!) ?? []) {
      if (c.pid === pid) continue;
      descendants.push(brief(c));
      queue.push(c.pid);
    }
  }
  // Memory series: the last ≤15 min every 10 s, drifting back along the process's rate.
  const series: [number, number][] = [];
  const from = Math.max(p.started_at, now - HISTORY_S);
  for (let t = from - (from % 10); t <= now; t += 10) {
    if (t < p.started_at) continue;
    const back = ((now - t) / 60) * p.rate;
    const wobble = Math.sin(t / 37 + pid) * 3 * MB + Math.sin(t / 11) * MB;
    series.push([t, Math.max(MB, Math.round(p.mem - back + wobble))]);
  }
  return {
    row,
    command_line: p.args.length ? p.args.join(" ") : p.exe ?? p.name,
    cwd: p.uid === USER_UID ? p.cwd : null,
    ancestors,
    descendants,
    descendants_total: descendants.reduce((a, d) => a + d.reclaim, 0),
    series,
  };
}

async function kill(a: KillArgs): Promise<KillOutcome> {
  await delay(400);
  const i = procs.findIndex((p) => p.pid === a.pid);
  const p = procs[i];
  if (!p || p.started_at !== a.startedAt) throw "Процесс уже завершился или pid занят другим процессом — снимок устарел";
  if (p.policy.kind === "protected") throw `Снять нельзя: ${p.policy.reason}`;
  if (p.policy.kind === "confirm" && !a.confirm) throw `Нужно явное подтверждение: ${p.policy.reason}`;
  const reclaimed = Math.round(p.mem);
  procs.splice(i, 1);
  // Like the real backend: push a fresh dashboard right after the kill.
  const processes = current.processes.filter((r) => r.pid !== p.pid);
  current = { ...current, processes, groups: groupsOf(processes), suspects: current.suspects.filter((s) => s.pid !== p.pid) };
  setTimeout(publish, 0);
  const message = `${p.name} (${p.pid}) снят SIGTERM, освобождено ${fmtBytes(reclaimed)}`;
  journal.unshift({ source: "rmon", ts: Math.floor(Date.now() / 1000), text: `app: ${message} — «${a.reason}»`, raw: { requested_by: "app", ...a } });
  return { pid: p.pid, name: p.name, signal: "SIGTERM", exited: true, reclaimed, dry_run: false, message };
}

export function createMockBackend(): Backend {
  return {
    subscribe: async (cb) => {
      listeners.add(cb);
      ensureTimer();
      setTimeout(() => listeners.has(cb) && cb(current), 0);
      return () => {
        listeners.delete(cb);
        if (!listeners.size && timer) {
          clearInterval(timer);
          timer = null;
        }
      };
    },
    details,
    kill,
    readJournal: async () => {
      await delay(150);
      return journal.slice();
    },
  };
}

// Dev only: a re-executed mock would be a second fake machine next to the first (other pids,
// other start times). Reload the page instead of hot-swapping it.
if (import.meta.hot) import.meta.hot.accept(() => location.reload());
