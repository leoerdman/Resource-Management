import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { cmdHint, cwdShort, GENERIC_NAMES, fmtAge, fmtBytes, fmtGrowth, fmtPct, plural, STATE_LABEL } from "../format";
import type { KillTarget, ProcessRow, ProcKey } from "../types";
import type { KillRequest } from "./KillDialog";
import { KillButton, PolicyTag } from "./Kill";

const ROW_H = 24;
const OVERSCAN = 12;
const MB = 1024 * 1024;

type SortKey = "pid" | "name" | "mem" | "rss" | "growth" | "cpu" | "threads" | "age" | "state" | "policy";

const POLICY_ORDER = { free: 0, confirm: 1, protected: 2 } as const;

// Numeric/string projection per column; null sorts last regardless of direction.
const KEY_FN: Record<SortKey, (r: ProcessRow) => number | string | null> = {
  pid: (r) => r.pid,
  name: (r) => r.name.toLowerCase(),
  mem: (r) => r.footprint ?? r.rss,
  rss: (r) => r.rss,
  growth: (r) => r.growth_per_min,
  cpu: (r) => r.cpu_pct,
  threads: (r) => r.threads,
  age: (r) => -r.started_at,
  state: (r) => r.state,
  policy: (r) => POLICY_ORDER[r.policy.kind],
};

const COLUMNS: { key: SortKey | null; label: string; num?: boolean; title?: string; cls?: string }[] = [
  { key: "pid", label: "PID", num: true },
  { key: "name", label: "Имя" },
  { key: "mem", label: "Память", num: true, title: "phys_footprint (как в Activity Monitor); если недоступен — RSS" },
  { key: "rss", label: "RSS", num: true, cls: "c-opt" },
  { key: "growth", label: "Рост МБ/мин", num: true },
  { key: "cpu", label: "CPU %", num: true },
  { key: "threads", label: "Потоки", num: true, cls: "c-opt" },
  { key: "age", label: "Возраст", num: true },
  { key: "state", label: "Состояние" },
  { key: "policy", label: "Политика" },
  { key: null, label: "" },
];

export function processTarget(r: ProcessRow, from: string): KillTarget {
  return {
    pid: r.pid,
    name: r.name,
    app_group: r.app_group,
    started_at: r.started_at,
    policy: r.policy,
    reclaim: r.footprint ?? r.rss,
    reason: `вручную из окна (${from})`,
  };
}

interface Props {
  processes: ProcessRow[];
  now: number;
  onKill: (r: KillRequest) => void;
  onOpen: (k: ProcKey) => void;
  /** Bumped by ⌘F: focus the filter. */
  focusSignal: number;
}

export const ProcessesTab = memo(function ProcessesTab({ processes, now, onKill, onOpen, focusSignal }: Props) {
  const [filter, setFilter] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("mem");
  const [desc, setDesc] = useState(true);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(600);
  const scrollRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (focusSignal) {
      filterRef.current?.focus();
      filterRef.current?.select();
    }
  }, [focusSignal]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const q = filter.trim().toLowerCase();
  const filtered = useMemo(() => {
    if (!q) return processes;
    return processes.filter(
      (r) =>
        String(r.pid) === q ||
        r.name.toLowerCase().includes(q) ||
        r.app_group.toLowerCase().includes(q) ||
        // Правило — `ProcessRow::matches` в rmon-core (его же использует `rmon ps --filter`).
        r.args.join(" ").toLowerCase().includes(q) ||
        (r.cwd != null && r.cwd.toLowerCase().includes(q)),
    );
  }, [processes, q]);

  const sorted = useMemo(() => {
    const fn = KEY_FN[sortKey];
    const dir = desc ? -1 : 1;
    const withKey = filtered.map((r) => ({ r, k: fn(r) }));
    withKey.sort((a, b) => {
      if (a.k === b.k) return a.r.pid - b.r.pid;
      if (a.k === null) return 1;
      if (b.k === null) return -1;
      return (a.k < b.k ? -1 : 1) * dir;
    });
    return withKey.map((x) => x.r);
  }, [filtered, sortKey, desc]);

  const totalMem = useMemo(() => filtered.reduce((a, r) => a + (r.footprint ?? r.rss), 0), [filtered]);

  const onSort = useCallback(
    (key: SortKey) => {
      if (key === sortKey) setDesc((d) => !d);
      else {
        setSortKey(key);
        setDesc(key !== "name" && key !== "state");
      }
    },
    [sortKey],
  );

  // Hand-written windowing: only rows in view (+ overscan) are mounted.
  const n = sorted.length;
  const start = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const end = Math.min(n, Math.ceil((scrollTop + viewH) / ROW_H) + OVERSCAN);
  const visible = sorted.slice(start, end);

  return (
    <div className="procs">
      <div className="toolbar">
        <input
          ref={filterRef}
          className="filter"
          type="search"
          placeholder="Фильтр: имя, приложение, каталог, аргументы, pid  (⌘F)"
          value={filter}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              setFilter("");
              e.currentTarget.blur();
            }
          }}
          onChange={(e) => {
            setFilter(e.target.value);
            scrollRef.current?.scrollTo({ top: 0 });
          }}
          spellCheck={false}
          autoCorrect="off"
        />
        <span className="dim">
          {q ? `${n} из ${processes.length}` : n} {plural(n, "процесс", "процесса", "процессов")} · {fmtBytes(totalMem)}
        </span>
      </div>
      <div className="ptable" ref={scrollRef} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}>
        <div className="prow phead" role="row">
          {COLUMNS.map((c, i) => (
            <div
              key={i}
              role="columnheader"
              className={`${c.num ? "num" : ""} ${c.cls ?? ""} ${c.key ? "sortable" : ""} ${c.key === sortKey ? "sorted" : ""}`}
              onClick={c.key ? () => onSort(c.key!) : undefined}
              title={c.title}
            >
              {c.label}
              {c.key === sortKey && <span className="arrow">{desc ? "▾" : "▴"}</span>}
            </div>
          ))}
        </div>
        <div className="pbody" style={{ height: n * ROW_H }}>
          <div style={{ transform: `translateY(${start * ROW_H}px)` }}>
            {visible.map((r) => (
              <Row key={r.pid} r={r} now={now} onKill={onKill} onOpen={onOpen} />
            ))}
          </div>
        </div>
        {!n && <div className="empty">Ничего не найдено</div>}
      </div>
    </div>
  );
});

const Row = memo(function Row({ r, now, onKill, onOpen }: { r: ProcessRow; now: number; onKill: (r: KillRequest) => void; onOpen: (k: ProcKey) => void }) {
  const g = r.growth_per_min;
  const cmd = r.args.length ? r.args.join(" ") : r.exe ?? r.name;
  const hint = GENERIC_NAMES.has(r.name) ? cmdHint(r.name, r.args) : "";
  const dir = r.cwd ? cwdShort(r.cwd).replace(/^~$/, "") : "";
  return (
    <div className={`prow st-${r.state}`} role="row" onClick={() => onOpen(r)}>
      <div className="num dim">{r.pid}</div>
      <div className="name" title={r.cwd ? `${r.cwd}\n${cmd}` : cmd}>
        {r.name}
        {dir && <span className="cwd-chip">{dir}</span>}
        {hint ? <span className="dim"> {hint}</span> : r.app_group !== r.name && <span className="dim"> · {r.app_group}</span>}
      </div>
      <div className="num strong" title={r.footprint == null ? "footprint недоступен (нет прав), показан RSS" : undefined}>
        {fmtBytes(r.footprint ?? r.rss)}
        {r.footprint == null && <span className="dim">*</span>}
      </div>
      <div className="num dim c-opt">{fmtBytes(r.rss)}</div>
      <div className={`num ${g != null && g >= 10 * MB ? "grow-hot" : g != null && g >= MB ? "grow-warm" : "dim"}`}>
        {fmtGrowth(g)}
      </div>
      <div className={`num ${r.cpu_pct >= 50 ? "grow-warm" : ""}`}>{fmtPct(r.cpu_pct)}</div>
      <div className="num dim c-opt">{r.threads}</div>
      <div className="num dim">{fmtAge(r.started_at, now)}</div>
      <div className="dim">{STATE_LABEL[r.state]}</div>
      <div>
        <PolicyTag policy={r.policy} />
      </div>
      <div className="act">
        {r.policy.kind !== "protected" && <KillButton target={processTarget(r, "Процессы")} onKill={onKill} />}
      </div>
    </div>
  );
});
