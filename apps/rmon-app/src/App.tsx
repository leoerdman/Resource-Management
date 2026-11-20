import { useCallback, useEffect, useRef, useState } from "react";
import { backend, type OpenTab } from "./api";
import { errText } from "./format";
import type { Dashboard, ProcKey } from "./types";
import { Header } from "./components/Header";
import { SuspectsTab } from "./components/SuspectsTab";
import { AppsTab } from "./components/AppsTab";
import { ProcessesTab } from "./components/ProcessesTab";
import { JournalTab } from "./components/JournalTab";
import { KillDialog, type KillRequest, type KillResult } from "./components/KillDialog";
import { DetailsDrawer } from "./components/DetailsDrawer";
import { Toasts, type Toast } from "./components/Toasts";
import { GuardTab } from "./components/GuardTab";
import { SettingsTab } from "./components/SettingsTab";

type Tab = "suspects" | "apps" | "procs" | "journal" | "guard" | "settings";

const TABS: { id: Tab; label: string }[] = [
  { id: "suspects", label: "Подозреваемые" },
  { id: "apps", label: "Приложения" },
  { id: "procs", label: "Процессы" },
  { id: "journal", label: "Журнал" },
  { id: "guard", label: "Сторож" },
  { id: "settings", label: "Настройки" },
];

const OPEN_TAB: Record<OpenTab, Tab> = { settings: "settings", guard: "guard", suspects: "suspects" };

function isTyping(t: EventTarget | null): boolean {
  return t instanceof HTMLElement && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName));
}

export default function App() {
  const [dash, setDash] = useState<Dashboard | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("suspects");
  const [killReq, setKillReq] = useState<KillRequest | null>(null);
  const [details, setDetails] = useState<ProcKey | null>(null);
  const [filterFocus, setFilterFocus] = useState(0);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastId = useRef(0);

  // Subscribe to dashboards. The backend only pushes while the window is visible; the hidden
  // check here also covers the browser mock.
  useEffect(() => {
    let alive = true;
    let off: (() => void) | null = null;
    let pending: Dashboard | null = null;
    const apply = (d: Dashboard) => {
      if (!alive) return;
      if (document.hidden) pending = d;
      else setDash(d);
    };
    const onVisible = () => {
      if (!document.hidden && pending) {
        setDash(pending);
        pending = null;
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    backend()
      .then((b) => b.subscribe(apply))
      .then((unsub) => {
        if (alive) off = unsub;
        else unsub();
      })
      .catch((e) => alive && setLoadError(errText(e)));
    return () => {
      alive = false;
      off?.();
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);

  // The menu-bar icon asks for a tab («Настройки…», «Сторож…»).
  useEffect(() => {
    let alive = true;
    let off: (() => void) | null = null;
    backend()
      .then((b) => b.onOpenTab((t) => OPEN_TAB[t] && setTab(OPEN_TAB[t])))
      .then((unsub) => {
        if (alive) off = unsub;
        else unsub();
      })
      .catch(() => {});
    return () => {
      alive = false;
      off?.();
    };
  }, []);

  // Keyboard: ⌘F → filter in «Процессы», ⌘, → «Настройки», 1–6 → tabs.
  const modalOpen = killReq != null;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (modalOpen) return;
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setTab("procs");
        setFilterFocus((n) => n + 1);
        return;
      }
      if (e.metaKey && !e.altKey && e.key === ",") {
        e.preventDefault();
        setTab("settings");
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target)) return;
      const i = ["1", "2", "3", "4", "5", "6"].indexOf(e.key);
      if (i >= 0) setTab(TABS[i].id);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [modalOpen]);

  const pushToast = useCallback((ok: boolean, text: string) => {
    const id = ++toastId.current;
    setToasts((ts) => [...ts, { id, ok, text }]);
    setTimeout(() => setToasts((ts) => ts.filter((t) => t.id !== id)), ok ? 6000 : 10000);
  }, []);

  const dismissToast = useCallback((id: number) => setToasts((ts) => ts.filter((t) => t.id !== id)), []);

  const onKill = useCallback((req: KillRequest) => {
    const targets = req.targets.filter((t) => t.policy.kind !== "protected");
    if (targets.length) setKillReq({ ...req, targets });
  }, []);

  const onOpen = useCallback((k: ProcKey) => setDetails({ pid: k.pid, started_at: k.started_at }), []);
  const closeDetails = useCallback(() => setDetails(null), []);

  const onKillResult = useCallback(
    (r: KillResult) => {
      setKillReq(null);
      pushToast(r.ok, r.text);
      // The backend pushes a fresh dashboard after a kill; only the open drawer needs care.
      setDetails((d) => (d && r.killed.some((k) => k.pid === d.pid && k.started_at === d.started_at) ? null : d));
    },
    [pushToast],
  );

  if (!dash) {
    return (
      <div className="splash">
        {loadError ? <span className="error">Не удалось получить данные: {loadError}</span> : "Загрузка…"}
      </div>
    );
  }

  return (
    <div className="app">
      <Header system={dash.system} history={dash.history} takenAt={dash.taken_at} />
      <nav className="tabs" role="tablist">
        {TABS.map((t, i) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            className={tab === t.id ? "tab active" : "tab"}
            onClick={() => setTab(t.id)}
            title={t.id === "settings" ? `Клавиша ${i + 1} или ⌘,` : `Клавиша ${i + 1}`}
          >
            {t.label}
            {t.id === "suspects" && dash.suspects.length > 0 && <span className="count">{dash.suspects.length}</span>}
          </button>
        ))}
      </nav>
      <main className="content">
        {tab === "suspects" && <SuspectsTab suspects={dash.suspects} processes={dash.processes} onKill={onKill} onOpen={onOpen} />}
        {tab === "apps" && (
          <AppsTab
            groups={dash.groups}
            processes={dash.processes}
            total={dash.system.total}
            now={dash.taken_at}
            onKill={onKill}
            onOpen={onOpen}
          />
        )}
        {tab === "procs" && (
          <ProcessesTab processes={dash.processes} now={dash.taken_at} onKill={onKill} onOpen={onOpen} focusSignal={filterFocus} />
        )}
        {tab === "journal" && <JournalTab />}
        {tab === "guard" && <GuardTab onToast={pushToast} />}
        {tab === "settings" && <SettingsTab onToast={pushToast} />}
      </main>
      {details && (
        <DetailsDrawer
          proc={details}
          blocked={modalOpen}
          onClose={closeDetails}
          onOpen={onOpen}
          onKill={onKill}
          onToast={pushToast}
        />
      )}
      {killReq && <KillDialog req={killReq} onCancel={() => setKillReq(null)} onResult={onKillResult} />}
      <Toasts toasts={toasts} onDismiss={dismissToast} />
    </div>
  );
}
