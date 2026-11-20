import type { JournalEntry, KillArgs, KillOutcome, State } from "./types";

export interface Backend {
  getState(): Promise<State>;
  /** Subscribe to state pushes (every 2 s). Resolves to an unsubscribe function. */
  onState(cb: (s: State) => void): Promise<() => void>;
  kill(args: KillArgs): Promise<KillOutcome>;
  readJournal(): Promise<JournalEntry[]>;
}

export const IS_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const [{ invoke }, { listen }] = await Promise.all([
    import("@tauri-apps/api/core"),
    import("@tauri-apps/api/event"),
  ]);
  return {
    getState: () => invoke<State>("get_state"),
    onState: (cb) => listen<State>("rmon://state", (e) => cb(e.payload)),
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
