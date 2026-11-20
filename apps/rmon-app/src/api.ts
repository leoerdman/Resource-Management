import type { Dashboard, JournalEntry, KillArgs, KillOutcome, ProcessDetails } from "./types";

export interface Backend {
  /**
   * Subscribe to dashboards: the latest one arrives right away, then every 2 s (only while the
   * window is visible) and right after a kill. Resolves to an unsubscribe function.
   */
  subscribe(cb: (d: Dashboard) => void): Promise<() => void>;
  details(pid: number, startedAt: number): Promise<ProcessDetails>;
  kill(args: KillArgs): Promise<KillOutcome>;
  readJournal(): Promise<JournalEntry[]>;
}

export const IS_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const { Channel, invoke } = await import("@tauri-apps/api/core");
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
