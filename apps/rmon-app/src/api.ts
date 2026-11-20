import type { Dashboard, GuardEvent, GuardStatus, JournalEntry, KillArgs, KillOutcome, ProcessDetails, Settings } from "./types";

/** Rust `Result<KillOutcome, String>` as serde sends it. */
export type KillManyResult = { Ok: KillOutcome } | { Err: string };

/** Tabs the menu-bar icon may ask to open. */
export type OpenTab = "settings" | "guard" | "suspects";

export interface Backend {
  /**
   * Subscribe to dashboards: the latest one arrives right away, then every 2 s (only while the
   * window is visible) and right after a kill. Resolves to an unsubscribe function.
   */
  subscribe(cb: (d: Dashboard) => void): Promise<() => void>;
  details(pid: number, startedAt: number): Promise<ProcessDetails>;
  kill(args: KillArgs): Promise<KillOutcome>;
  readJournal(): Promise<JournalEntry[]>;
  /** Kill several at once (in parallel on the backend, one dashboard push). Results follow `targets`. */
  killMany(targets: [number, number][], confirm: boolean, reason: string): Promise<KillManyResult[]>;

  guardStatus(): Promise<GuardStatus>;
  /** Every guard event while the window is visible. Resolves to an unsubscribe function. */
  onGuardEvent(cb: (e: GuardEvent) => void): Promise<() => void>;

  getSettings(): Promise<Settings>;
  /** Validated by the backend; rejects with a Russian message. Applied immediately. */
  saveSettings(settings: Settings): Promise<Settings>;
  /** Open config.toml in the default editor. */
  openConfig(): Promise<void>;
  /** The menu-bar icon asks the window to show a tab. */
  onOpenTab(cb: (tab: OpenTab) => void): Promise<() => void>;
}

export const IS_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const [{ Channel, invoke }, { listen }] = await Promise.all([
    import("@tauri-apps/api/core"),
    import("@tauri-apps/api/event"),
  ]);
  return {
    subscribe: async (cb) => {
      let live = true;
      const ch = new Channel<Dashboard>();
      ch.onmessage = (d) => {
        if (live) cb(d);
      };
      // The backend keeps one channel: a new subscribe replaces the old one.
      await invoke("subscribe", { channel: ch });
      return () => {
        live = false;
      };
    },
    details: (pid, startedAt) => invoke<ProcessDetails>("process_details", { pid, startedAt }),
    kill: (args) => invoke<KillOutcome>("kill_process", { ...args }),
    readJournal: () => invoke<JournalEntry[]>("read_journal"),
    killMany: (targets, confirm, reason) => invoke<KillManyResult[]>("kill_processes", { targets, confirm, reason }),
    guardStatus: () => invoke<GuardStatus>("guard_status"),
    onGuardEvent: (cb) => listen<GuardEvent>("rmon://guard", (e) => cb(e.payload)),
    getSettings: () => invoke<Settings>("get_settings"),
    saveSettings: (settings) => invoke<Settings>("save_settings", { settings }),
    openConfig: () => invoke<void>("open_config"),
    onOpenTab: (cb) => listen<string>("rmon://open-tab", (e) => cb(e.payload as OpenTab)),
  };
}

let backendP: Promise<Backend> | null = null;

/** Real Tauri backend inside the app; a ticking mock in a plain browser (`npm run dev`). */
export function backend(): Promise<Backend> {
  if (!backendP) {
    backendP = IS_TAURI ? tauriBackend() : import("./mock").then((m) => m.createMockBackend());
  }
  return backendP;
}

// Dev only: never keep two backends alive side by side after a hot update.
if (import.meta.hot) import.meta.hot.accept(() => location.reload());
