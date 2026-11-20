import { memo, useMemo } from "react";
import { cmdHint, fmtBytes, KIND_LABEL, plural, POLICY_LABEL } from "../format";
import type { KillTarget, ProcessRow, Suspect } from "../types";
import { KillButton, LockIcon } from "./Kill";

interface Props {
  suspects: Suspect[];
  processes: ProcessRow[];
  onKill: (t: KillTarget) => void;
}

export const SuspectsTab = memo(function SuspectsTab({ suspects, processes, onKill }: Props) {
  const { sorted, reclaimable } = useMemo(() => {
    const sorted = [...suspects].sort((a, b) => b.score - a.score || b.reclaim - a.reclaim);
    // One process can be a suspect for several reasons — count its memory once.
    const seen = new Set<number>();
    let reclaimable = 0;
    for (const s of sorted) {
      if (s.policy.kind === "protected" || seen.has(s.pid)) continue;
      seen.add(s.pid);
      reclaimable += s.reclaim;
    }
    return { sorted, reclaimable };
  }, [suspects]);

  // Suspects carry no args; look them up so "node" cards say what that node runs.
  const hints = useMemo(() => {
    const want = new Set(suspects.map((s) => s.pid));
    const m = new Map<number, string>();
    for (const r of processes) if (want.has(r.pid) && r.app_group === r.name) m.set(r.pid, cmdHint(r.name, r.args));
    return m;
  }, [suspects, processes]);

  if (!sorted.length) return <div className="empty">Подозреваемых нет — с памятью всё в порядке.</div>;

  return (
    <div className="pane-scroll">
      <div className="toolbar">
        <span className="dim">
          {sorted.length} {plural(sorted.length, "находка", "находки", "находок")}, снятие освободит до{" "}
          <b>{fmtBytes(reclaimable)}</b>
        </span>
      </div>
      <div className="cards">
        {sorted.map((s) => (
          <SuspectCard key={`${s.pid}-${s.kind}`} s={s} hint={hints.get(s.pid) ?? ""} onKill={onKill} />
        ))}
      </div>
    </div>
  );
});

const SuspectCard = memo(function SuspectCard({ s, hint, onKill }: { s: Suspect; hint: string; onKill: (t: KillTarget) => void }) {
  const target: KillTarget = {
    pid: s.pid,
    name: s.name,
    app_group: s.app_group,
    started_at: s.started_at,
    policy: s.policy,
    reclaim: s.reclaim,
    reason: `окно, подозреваемый «${KIND_LABEL[s.kind]}»: ${s.reason}`,
  };
  return (
    <article className={`card kind-${s.kind}`}>
      <div className="card-head">
        <span className="card-name" title={`pid ${s.pid}`}>
          {s.name} <span className="dim">{s.pid}</span>
        </span>
        <span className={`badge badge-${s.kind}`}>{KIND_LABEL[s.kind]}</span>
        <span className="score" title={`Оценка ${s.score} из 100`}>
          <span style={{ width: `${s.score}%` }} />
        </span>
      </div>
      {(s.app_group !== s.name || hint) && (
        <div className="dim small ellipsis">{[s.app_group !== s.name ? s.app_group : "", hint].filter(Boolean).join(" · ")}</div>
      )}
      <p className="reason">{s.reason}</p>
      <div className="card-foot">
        {s.reclaim > 0 && (
          <span>
            освободит <b>{fmtBytes(s.reclaim)}</b>
          </span>
        )}
        {s.policy.kind === "protected" ? (
          <span className="pol-protected small">
            <LockIcon /> {s.policy.reason}
          </span>
        ) : (
          <span className={`pol-${s.policy.kind} small`} title={s.policy.kind === "confirm" ? s.policy.reason : undefined}>
            {POLICY_LABEL[s.policy.kind]}
          </span>
        )}
        <span className="spacer" />
        <KillButton target={target} onKill={onKill} />
      </div>
    </article>
  );
});
