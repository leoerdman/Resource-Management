import { useCallback, useEffect, useRef, useState } from "react";
import { backend } from "./api";
import { errText } from "./format";
import type { KillTarget, State } from "./types";
import { Header } from "./components/Header";
import { SuspectsTab } from "./components/SuspectsTab";
import { AppsTab } from "./components/AppsTab";
import { ProcessesTab } from "./components/ProcessesTab";
import { JournalTab } from "./components/JournalTab";
import { KillDialog, type KillResult } from "./components/KillDialog";
import { Toasts, type Toast } from "./components/Toasts";

type Tab = "suspects" | "apps" | "procs" | "journal";

const TABS: { id: Tab; label: string }[] = [
  { id: "suspects", label: "Подозреваемые" },
  { id: "apps", label: "Приложения" },
  { id: "procs", label: "Процессы" },
  { id: "journal", label: "Журнал" },
];

export default function App() {
  const [state, setState] = useState<State | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("suspects");
  const [killTarget, setKillTarget] = useState<KillTarget | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastId = useRef(0);

  // Subscribe to state pushes. While the window is hidden we keep only the latest
  // payload and skip rendering, so a backgrounded window costs ~nothing.
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | null = null;
    let pending: State | null = null;
    const apply = (s: State) => {
      if (!alive) return;
      if (document.hidden) pending = s;
      else setState(s);
    };
    const onVisible = () => {
      if (!document.hidden && pending) {
        setState(pending);
        pending = null;
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    backend()
      .then(async (b) => {
        const off = await b.onState(apply);
        if (!alive) off();
        else unlisten = off;
        apply(await b.getState());
      })
      .catch((e) => alive && setLoadError(errText(e)));
    return () => {
      alive = false;
      unlisten?.();
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);

  const pushToast = useCallback((ok: boolean, text: string) => {
    const id = ++toastId.current;
    setToasts((ts) => [...ts, { id, ok, text }]);
    setTimeout(() => setToasts((ts) => ts.filter((t) => t.id !== id)), ok ? 6000 : 10000);
  }, []);

  const dismissToast = useCallback((id: number) => setToasts((ts) => ts.filter((t) => t.id !== id)), []);

  const onKill = useCallback((t: KillTarget) => {
    if (t.policy.kind !== "protected") setKillTarget(t);
  }, []);

  const onKillResult = useCallback(
    (r: KillResult) => {
      setKillTarget(null);
      pushToast(r.ok, r.text);
      if (r.ok) {
        // Pull a fresh snapshot right away instead of waiting for the next push.
        backend()
          .then((b) => b.getState())
          .then((s) => setState(s))
          .catch(() => {});
      }
    },
    [pushToast],
  );

  if (!state) {
    return (
      <div className="splash">
        {loadError ? <span className="error">Не удалось получить данные: {loadError}</span> : "Загрузка…"}
      </div>
    );
  }

  return (
    <div className="app">
      <Header system={state.system} history={state.history} takenAt={state.taken_at} />
      <nav className="tabs" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            className={tab === t.id ? "tab active" : "tab"}
            onClick={() => setTab(t.id)}
          >
            {t.label}
            {t.id === "suspects" && state.suspects.length > 0 && <span className="count">{state.suspects.length}</span>}
          </button>
        ))}
      </nav>
      <main className="content">
        {tab === "suspects" && <SuspectsTab suspects={state.suspects} processes={state.processes} onKill={onKill} />}
        {tab === "apps" && (
          <AppsTab groups={state.groups} processes={state.processes} total={state.system.total} now={state.taken_at} onKill={onKill} />
        )}
        {tab === "procs" && <ProcessesTab processes={state.processes} now={state.taken_at} onKill={onKill} />}
        {tab === "journal" && <JournalTab />}
      </main>
      {killTarget && <KillDialog target={killTarget} onCancel={() => setKillTarget(null)} onResult={onKillResult} />}
      <Toasts toasts={toasts} onDismiss={dismissToast} />
    </div>
  );
}
