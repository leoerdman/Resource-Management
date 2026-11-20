// Backend contract (mirrors rmon-core serde output). Sizes are bytes, times are unix seconds.

export type Pressure = "normal" | "warning" | "critical";
export type ProcState = "running" | "sleeping" | "stopped" | "zombie" | "unknown";

export type KillPolicy =
  | { kind: "protected"; reason: string }
  | { kind: "confirm"; reason: string }
  | { kind: "free" };

export interface VmCounters {
  pageins: number;
  pageouts: number;
  swapins: number;
  swapouts: number;
  compressions: number;
  decompressions: number;
}

export interface SystemMemory {
  total: number;
  used: number;
  app: number;
  wired: number;
  compressed: number;
  cached_files: number;
  free: number;
  swap_total: number;
  swap_used: number;
  pressure: Pressure;
  available_pct: number;
  counters: VmCounters;
}

export interface ProcessRow {
  pid: number;
  ppid: number;
  name: string;
  exe: string | null;
  args: string[];
  uid: number;
  footprint: number | null;
  rss: number;
  cpu_pct: number;
  threads: number;
  state: ProcState;
  started_at: number;
  app_group: string;
  /** bytes per minute, from history; null while there is not enough history */
  growth_per_min: number | null;
  policy: KillPolicy;
}

export interface AppGroup {
  name: string;
  pids: number[];
  footprint: number;
  count: number;
}

export type SuspectKind = "hog" | "growing" | "orphan" | "idle" | "duplicate" | "zombie";

export interface Suspect {
  pid: number;
  name: string;
  app_group: string;
  started_at: number;
  kind: SuspectKind;
  /** 0–100 */
  score: number;
  /** Russian, human-readable */
  reason: string;
  reclaim: number;
  policy: KillPolicy;
}

export interface HistoryPoint {
  t: number;
  used: number;
  swap_used: number;
  compressed: number;
  pressure: Pressure;
}

export interface State {
  taken_at: number;
  system: SystemMemory;
  /** up to 15 min */
  history: HistoryPoint[];
  groups: AppGroup[];
  processes: ProcessRow[];
  suspects: Suspect[];
}

export interface KillOutcome {
  pid: number;
  name: string;
  signal: "SIGTERM" | "SIGKILL";
  exited: boolean;
  reclaimed: number;
  dry_run: boolean;
  message: string;
}

export interface JournalEntry {
  source: "rmon" | "memory-guard";
  ts: number | null;
  text: string;
  raw: unknown;
}

export interface KillArgs {
  pid: number;
  startedAt: number;
  confirm: boolean;
  reason: string;
}

/** What the kill dialog needs to know about the target (built from a Suspect or a ProcessRow). */
export interface KillTarget {
  pid: number;
  name: string;
  app_group: string;
  started_at: number;
  policy: KillPolicy;
  reclaim: number;
  /** Text recorded in the action log as the reason. */
  reason: string;
}
