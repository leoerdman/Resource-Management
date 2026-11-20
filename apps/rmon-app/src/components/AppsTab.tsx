import { memo, useCallback, useMemo, useState } from "react";
import { cmdHint, GENERIC_NAMES, fmtAge, fmtBytes, fmtGrowth, fmtPct, plural } from "../format";
import type { AppGroup, ProcessRow, ProcKey } from "../types";
import type { KillRequest } from "./KillDialog";
import { KillButton, PolicyTag } from "./Kill";
import { processTarget } from "./ProcessesTab";

interface Props {
  groups: AppGroup[];
  processes: ProcessRow[];
  total: number;
  now: number;
  onKill: (r: KillRequest) => void;
  onOpen: (k: ProcKey) => void;
}

export const AppsTab = memo(function AppsTab({ groups, processes, total, now, onKill, onOpen }: Props) {
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());

  const sorted = useMemo(() => [...groups].sort((a, b) => b.footprint - a.footprint), [groups]);
  const byPid = useMemo(() => {
    const m = new Map<number, ProcessRow>();
    for (const r of processes) m.set(r.pid, r);
    return m;
  }, [processes]);

  const toggle = useCallback((name: string) => {
    setOpen((s) => {
      const next = new Set(s);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  }, []);

  return (
    <div className="pane-scroll">
      <div className="groups">
        {sorted.map((g) => (
          <GroupRow
            key={g.name}
            g={g}
            total={total}
            expanded={open.has(g.name)}
            rows={open.has(g.name) ? g.pids : null}
            byPid={byPid}
            now={now}
            onToggle={toggle}
            onKill={onKill}
            onOpen={onOpen}
          />
        ))}
      </div>
    </div>
  );
});

interface RowProps {
  g: AppGroup;
  total: number;
  expanded: boolean;
  rows: number[] | null;
  byPid: Map<number, ProcessRow>;
  now: number;
  onToggle: (name: string) => void;
  onKill: (r: KillRequest) => void;
  onOpen: (k: ProcKey) => void;
}

function GroupRow({ g, total, expanded, rows, byPid, now, onToggle, onKill, onOpen }: RowProps) {
  const share = total ? (g.footprint / total) * 100 : 0;
  const procs = useMemo(() => {
    if (!rows) return null;
    const list: ProcessRow[] = [];
    for (const pid of rows) {
      const r = byPid.get(pid);
      if (r) list.push(r);
    }
    return list.sort((a, b) => (b.footprint ?? b.rss) - (a.footprint ?? a.rss));
  }, [rows, byPid]);

  return (
    <div className={expanded ? "group open" : "group"}>
      <button className="group-head" onClick={() => onToggle(g.name)} aria-expanded={expanded}>
        <span className="chev">{expanded ? "▾" : "▸"}</span>
        <span className="group-name">{g.name}</span>
        <span className="group-count dim">
          {g.count} {plural(g.count, "процесс", "процесса", "процессов")}
        </span>
        <span className="share">
          <span style={{ width: `${Math.min(100, share)}%` }} />
        </span>
        <span className="group-pct dim">{fmtPct(share)} %</span>
        <span className="group-mem">{fmtBytes(g.footprint)}</span>
      </button>
      {procs && (
        <div className="group-procs">
          {procs.map((r) => (
            <div key={r.pid} className="gproc" onClick={() => onOpen(r)}>
              <span className="num dim">{r.pid}</span>
              <span className="name" title={r.args.join(" ") || r.exe || r.name}>
                {r.name}
                {GENERIC_NAMES.has(r.name) && <span className="dim"> {cmdHint(r.name, r.args)}</span>}
              </span>
              <span className="num strong">{fmtBytes(r.footprint ?? r.rss)}</span>
              <span className="num dim" title="Рост МБ/мин">
                {fmtGrowth(r.growth_per_min)}
              </span>
              <span className="num dim" title="CPU %">
                {fmtPct(r.cpu_pct)} %
              </span>
              <span className="num dim">{fmtAge(r.started_at, now)}</span>
              <PolicyTag policy={r.policy} />
              <span className="act">
                {r.policy.kind !== "protected" && <KillButton target={processTarget(r, "Приложения")} onKill={onKill} />}
              </span>
            </div>
          ))}
          {!procs.length && <div className="dim small">Процессы уже завершились</div>}
        </div>
      )}
    </div>
  );
}
