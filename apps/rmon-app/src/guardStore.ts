// Состояние сторожа памяти — одно на окно: его читают вкладка «Сторож» и переключатель
// в полосе вкладок. Опрос (раз в 3 с, чтобы тикало «красное держится N с») и подписка на
// события сторожа идут, только пока есть хоть один подписчик.
import { useSyncExternalStore } from "react";
import { backend } from "./api";
import { errText } from "./format";
import type { GuardSettings, GuardStatus } from "./types";

export interface GuardState {
  status: GuardStatus | null;
  /** Флаги сторожа из настроек (что сохранено в config.toml). */
  settings: GuardSettings | null;
  error: string | null;
  saving: boolean;
}

const POLL_MS = 3000;

let state: GuardState = { status: null, settings: null, error: null, saving: false };
const listeners = new Set<() => void>();
let stop: (() => void) | null = null;

function set(patch: Partial<GuardState>) {
  state = { ...state, ...patch };
  for (const l of listeners) l();
}

async function refresh() {
  try {
    const b = await backend();
    set({ status: await b.guardStatus(), error: null });
  } catch (e) {
    set({ error: errText(e) });
  }
}

function start() {
  let alive = true;
  let off: (() => void) | null = null;
  backend()
    .then(async (b) => {
      const s = await b.getSettings();
      if (alive) set({ settings: s.guard });
      const unsub = await b.onGuardEvent(() => void refresh());
      if (alive) off = unsub;
      else unsub();
    })
    .catch((e) => alive && set({ error: errText(e) }));
  void refresh();
  const id = setInterval(() => void refresh(), POLL_MS);
  return () => {
    alive = false;
    clearInterval(id);
    off?.();
  };
}

function subscribe(l: () => void) {
  listeners.add(l);
  if (listeners.size === 1) stop = start();
  return () => {
    listeners.delete(l);
    if (listeners.size === 0) {
      stop?.();
      stop = null;
    }
  };
}

/** Текущее состояние сторожа; компонент перерисуется при изменении. */
export function useGuard(): GuardState {
  return useSyncExternalStore(subscribe, () => state);
}

export const guardStore = {
  refresh,
  /** Статус пришёл из другой команды (перенос старого сторожа) — показать сразу. */
  setStatus(status: GuardStatus) {
    set({ status });
  },
  /**
   * Поменять флаги сторожа: запись в config.toml через `save_settings` (бэкенд проверяет и
   * применяет сразу). Отказ — исключение со строкой по-русски.
   */
  async patch(patch: Partial<GuardSettings>): Promise<GuardSettings> {
    set({ saving: true });
    try {
      const b = await backend();
      const cur = await b.getSettings();
      const saved = await b.saveSettings({ ...cur, guard: { ...cur.guard, ...patch } });
      set({ settings: saved.guard });
      await refresh();
      return saved.guard;
    } finally {
      set({ saving: false });
    }
  },
};

/** Короткое имя режима для полосы вкладок и цвет точки. */
export function modeBrief(s: GuardStatus | null, settings: GuardSettings | null): { label: string; tone: "on" | "dry" | "off" | "wait" } {
  const kind = s?.mode.kind ?? (settings?.enabled ? "armed" : "off");
  switch (kind) {
    case "armed":
      return { label: "сторожит", tone: "on" };
    case "dry_run":
      return { label: "проба", tone: "dry" };
    case "standby":
      return { label: "ждёт", tone: "wait" };
    case "broken":
      return { label: "стоит", tone: "wait" };
    default:
      return { label: "выключен", tone: "off" };
  }
}
