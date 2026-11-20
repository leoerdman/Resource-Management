import { memo, useCallback, useMemo, useState, type ReactNode } from "react";
import { cmdHint, fmtBytes, GENERIC_NAMES, KIND_LABEL, plural, POLICY_LABEL } from "../format";
import type { KillPolicy, KillTarget, ProcKey, Suspect, SuspectKind } from "../types";
import { KillButton, LockIcon, PolicyTag } from "./Kill";
import type { KillRequest } from "./KillDialog";

/** One process, with every reason it is suspected for. */
interface Item {
  key: string;
  pid: number;
  name: string;
  app_group: string;
  cmd: string;
  hint: string;
  started_at: number;
  policy: KillPolicy;
  reclaim: number;
  score: number;
  kinds: SuspectKind[];
  reasons: string[];
  group: string | null;
}

/** Abandoned copies of one command (same `Suspect.group`). */
interface Group {
  key: string;
  items: Item[];
  name: string;
  hint: string;
  score: number;
  reclaim: number;
  kinds: SuspectKind[];
  policy: KillPolicy;
}

type Entry = { t: "one"; it: Item } | { t: "group"; g: Group };
type SortKey = "score" | "mem" | "name";
type View = "list" | "cards";

const VIEW_KEY = "rmon.suspects.view";

function loadView(): View {
  try {
    return localStorage.getItem(VIEW_KEY) === "cards" ? "cards" : "list";
  } catch {
    return "list";
  }
}

function hintFor(name: string, cmd: string): string {
  return GENERIC_NAMES.has(name) ? cmdHint(name, cmd) : "";
}

function buildEntries(suspects: Suspect[]): Entry[] {
  const byProc = new Map<string, Item>();
  for (const s of suspects) {
    const key = `${s.pid}:${s.started_at}`;
    const it = byProc.get(key);
    if (it) {
      if (!it.kinds.includes(s.kind)) it.kinds.push(s.kind);
      it.reasons.push(s.reason);
      it.score = Math.max(it.score, s.score);
      it.reclaim = Math.max(it.reclaim, s.reclaim);
      it.group ??= s.group;
    } else {
      byProc.set(key, {
        key,
        pid: s.pid,
        name: s.name,
        app_group: s.app_group,
        cmd: s.cmd,
        hint: hintFor(s.name, s.cmd),
        started_at: s.started_at,
        policy: s.policy,
        reclaim: s.reclaim,
        score: s.score,
        kinds: [s.kind],
        reasons: [s.reason],
        group: s.group,
      });
    }
  }
  const groups = new Map<string, Item[]>();
  const out: Entry[] = [];
  for (const it of byProc.values()) {
    if (it.group == null) out.push({ t: "one", it });
    else {
      const list = groups.get(it.group);
      if (list) list.push(it);
      else groups.set(it.group, [it]);
    }
  }
  for (const [key, items] of groups) {
    if (items.length === 1) {
      out.push({ t: "one", it: items[0] });
      continue;
    }
    items.sort((a, b) => b.reclaim - a.reclaim);
    const kinds: SuspectKind[] = [];
    for (const it of items) for (const k of it.kinds) if (!kinds.includes(k)) kinds.push(k);
    // The strictest policy among the copies speaks for the group.
    const policy =
      items.find((i) => i.policy.kind === "protected")?.policy ??
      items.find((i) => i.policy.kind === "confirm")?.policy ??
      items[0].policy;
    out.push({
      t: "group",
      g: {
        key,
        items,
        name: items[0].name,
        hint: hintFor(items[0].name, items[0].cmd) || items[0].cmd.slice(0, 60),
        score: Math.max(...items.map((i) => i.score)),
        reclaim: items.reduce((a, i) => a + i.reclaim, 0),
        kinds,
        policy,
      },
    });
  }
  return out;
}

const entryScore = (e: Entry) => (e.t === "one" ? e.it.score : e.g.score);
const entryMem = (e: Entry) => (e.t === "one" ? e.it.reclaim : e.g.reclaim);
const entryName = (e: Entry) => (e.t === "one" ? e.it.name : e.g.name).toLowerCase();

function targetOf(it: Item): KillTarget {
  return {
    pid: it.pid,
    name: it.name,
    app_group: it.app_group,
    started_at: it.started_at,
    policy: it.policy,
    reclaim: it.reclaim,
    reason: `окно, подозреваемый «${it.kinds.map((k) => KIND_LABEL[k]).join(", ")}»: ${it.reasons[0]}`,
  };
}

function groupTitle(g: Group): string {
  return `${g.items.length} ${plural(g.items.length, "брошенная копия", "брошенные копии", "брошенных копий")} «${g.hint}»`;
}

function killGroup(g: Group, onKill: (r: KillRequest) => void) {
  onKill({ targets: g.items.filter((i) => i.policy.kind !== "protected").map(targetOf), title: groupTitle(g) });
}

interface Props {
  suspects: Suspect[];
  onKill: (r: KillRequest) => void;
  onOpen: (k: ProcKey) => void;
}

export const SuspectsTab = memo(function SuspectsTab({ suspects, onKill, onOpen }: Props) {
  const [view, setView] = useState<View>(loadView);
  const [sortKey, setSortKey] = useState<SortKey>("score");
  const [desc, setDesc] = useState(true);
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());

  const changeView = (v: View) => {
    setView(v);
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      /* storage may be unavailable — the choice just won't persist */
    }
  };

  const entries = useMemo(() => buildEntries(suspects), [suspects]);

  const sorted = useMemo(() => {
    const dir = desc ? -1 : 1;
    const key = sortKey === "score" ? entryScore : sortKey === "mem" ? entryMem : entryName;
    return [...entries].sort((a, b) => {
      const ka = key(a);
      const kb = key(b);
      if (ka !== kb) return (ka < kb ? -1 : 1) * dir;
      return entryMem(b) - entryMem(a);
    });
  }, [entries, sortKey, desc]);

  const reclaimable = useMemo(() => {
    let sum = 0;
    for (const e of entries) {
      const items = e.t === "one" ? [e.it] : e.g.items;
      for (const i of items) if (i.policy.kind !== "protected") sum += i.reclaim;
    }
    return sum;
  }, [entries]);

  const toggle = useCallback((key: string) => {
    setOpen((s) => {
      const next = new Set(s);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  const onSort = (k: SortKey) => {
    if (k === sortKey) setDesc((d) => !d);
    else {
      setSortKey(k);
      setDesc(k !== "name");
    }
  };

  const toolbar = (
    <div className="toolbar">
      <span className="dim">
        {suspects.length} {plural(suspects.length, "находка", "находки", "находок")}, снятие освободит до{" "}
        <b>{fmtBytes(reclaimable)}</b>
      </span>
      <span className="spacer" />
      <div className="seg-ctl" role="radiogroup" aria-label="Вид">
        <button role="radio" aria-checked={view === "cards"} className={view === "cards" ? "on" : ""} onClick={() => changeView("cards")}>
          Карточки
        </button>
        <button role="radio" aria-checked={view === "list"} className={view === "list" ? "on" : ""} onClick={() => changeView("list")}>
          Список
        </button>
      </div>
    </div>
  );

  if (!suspects.length) return <div className="empty">Подозреваемых нет — с памятью всё в порядке.</div>;

  return (
    <div className="pane-scroll">
      {toolbar}
      {view === "list" ? (
        <div className="slist" role="table">
          <div className="srow shead" role="row">
            <SortHead k="score" cur={sortKey} desc={desc} onSort={onSort}>
              Оценка
            </SortHead>
            <SortHead k="name" cur={sortKey} desc={desc} onSort={onSort}>
              Имя
            </SortHead>
            <div>Вид</div>
            <div>Строка запуска · причина</div>
            <SortHead k="mem" cur={sortKey} desc={desc} onSort={onSort} num>
              Освободит
            </SortHead>
            <div>Политика</div>
            <div />
          </div>
          {sorted.map((e) =>
            e.t === "one" ? (
              <ListRow key={e.it.key} it={e.it} onKill={onKill} onOpen={onOpen} />
            ) : (
              <ListGroup key={e.g.key} g={e.g} expanded={open.has(e.g.key)} onToggle={toggle} onKill={onKill} onOpen={onOpen} />
            ),
          )}
        </div>
      ) : (
        <div className="cards">
          {sorted.map((e) =>
            e.t === "one" ? (
              <Card key={e.it.key} it={e.it} onKill={onKill} onOpen={onOpen} />
            ) : (
              <GroupCard key={e.g.key} g={e.g} expanded={open.has(e.g.key)} onToggle={toggle} onKill={onKill} onOpen={onOpen} />
            ),
          )}
        </div>
      )}
    </div>
  );
});

function SortHead(props: { k: SortKey; cur: SortKey; desc: boolean; onSort: (k: SortKey) => void; num?: boolean; children: ReactNode }) {
  const on = props.k === props.cur;
  return (
    <div
      role="columnheader"
      className={`sortable ${on ? "sorted" : ""} ${props.num ? "num" : ""}`}
      onClick={() => props.onSort(props.k)}
    >
      {props.children}
      {on && <span className="arrow">{props.desc ? "▾" : "▴"}</span>}
    </div>
  );
}

function Badges({ kinds }: { kinds: SuspectKind[] }) {
  return (
    <>
      {kinds.map((k) => (
        <span key={k} className={`badge badge-${k}`}>
          {KIND_LABEL[k]}
        </span>
      ))}
    </>
  );
}

function ScoreBar({ score }: { score: number }) {
  return (
    <span className="score" title={`Оценка ${score} из 100`}>
      <span style={{ width: `${score}%` }} className={score >= 70 ? "hot" : score >= 45 ? "warm" : undefined} />
    </span>
  );
}

interface RowProps {
  it: Item;
  onKill: (r: KillRequest) => void;
  onOpen: (k: ProcKey) => void;
  sub?: boolean;
}

const ListRow = memo(function ListRow({ it, onKill, onOpen, sub }: RowProps) {
  const title = [it.cmd, ...it.reasons].join("\n");
  return (
    <div className={sub ? "srow sub" : "srow"} role="row" onClick={() => onOpen(it)}>
      <div className="score-cell">
        {!sub && <ScoreBar score={it.score} />}
        {!sub && <span className="dim small">{it.score}</span>}
      </div>
      <div className="ellipsis">
        <b>{it.name}</b> <span className="dim">{it.pid}</span>
      </div>
      <div className="badges">
        <Badges kinds={it.kinds} />
      </div>
      <div className="ellipsis" title={title}>
        {it.hint && <span className="hint">{it.hint} · </span>}
        {it.reasons.join("; ")}
      </div>
      <div className="num strong">{it.reclaim > 0 ? fmtBytes(it.reclaim) : "—"}</div>
      <div>
        <PolicyTag policy={it.policy} />
      </div>
      <div className="act">
        <KillButton target={targetOf(it)} onKill={onKill} />
      </div>
    </div>
  );
});

interface GroupProps {
  g: Group;
  expanded: boolean;
  onToggle: (key: string) => void;
  onKill: (r: KillRequest) => void;
  onOpen: (k: ProcKey) => void;
}

const ListGroup = memo(function ListGroup({ g, expanded, onToggle, onKill, onOpen }: GroupProps) {
  const killable = g.items.some((i) => i.policy.kind !== "protected");
  return (
    <>
      <div className="srow group-row" role="row" onClick={() => onToggle(g.key)} aria-expanded={expanded}>
        <div className="score-cell">
          <ScoreBar score={g.score} />
          <span className="dim small">{g.score}</span>
        </div>
        <div className="ellipsis">
          <span className="chev">{expanded ? "▾" : "▸"}</span> <b>{g.name}</b> <span className="dim">×{g.items.length}</span>
        </div>
        <div className="badges">
          <Badges kinds={g.kinds} />
        </div>
        <div className="ellipsis" title={[g.items[0].cmd, ...g.items[0].reasons].join("\n")}>
          {groupTitle(g)} — вместе {fmtBytes(g.reclaim)}
        </div>
        <div className="num strong">{fmtBytes(g.reclaim)}</div>
        <div>
          <PolicyTag policy={g.policy} />
        </div>
        <div className="act">
          {killable && (
            <button
              className={g.policy.kind === "confirm" ? "btn btn-kill btn-warn" : "btn btn-kill"}
              onClick={(e) => {
                e.stopPropagation();
                killGroup(g, onKill);
              }}
            >
              Снять все
            </button>
          )}
        </div>
      </div>
      {expanded && g.items.map((it) => <ListRow key={it.key} it={it} onKill={onKill} onOpen={onOpen} sub />)}
    </>
  );
});

const Card = memo(function Card({ it, onKill, onOpen }: RowProps) {
  return (
    <article className="card clickable" onClick={() => onOpen(it)}>
      <div className="card-head">
        <span className="card-name" title={`pid ${it.pid}`}>
          {it.name} <span className="dim">{it.pid}</span>
        </span>
        <Badges kinds={it.kinds} />
        <ScoreBar score={it.score} />
      </div>
      {(it.app_group !== it.name || it.hint) && (
        <div className="dim small ellipsis" title={it.cmd}>
          {[it.app_group !== it.name ? it.app_group : "", it.hint].filter(Boolean).join(" · ")}
        </div>
      )}
      {it.reasons.map((r, i) => (
        <p key={i} className="reason">
          {r}
        </p>
      ))}
      <div className="card-foot">
        {it.reclaim > 0 && (
          <span>
            освободит <b>{fmtBytes(it.reclaim)}</b>
          </span>
        )}
        <PolicyLine policy={it.policy} />
        <span className="spacer" />
        <KillButton target={targetOf(it)} onKill={onKill} />
      </div>
    </article>
  );
});

const GroupCard = memo(function GroupCard({ g, expanded, onToggle, onKill, onOpen }: GroupProps) {
  const killable = g.items.some((i) => i.policy.kind !== "protected");
  return (
    <article className="card card-group">
      <div className="card-head">
        <span className="card-name">
          {g.name} <span className="dim">×{g.items.length}</span>
        </span>
        <Badges kinds={g.kinds} />
        <ScoreBar score={g.score} />
      </div>
      <p className="reason" title={g.items[0].cmd}>
        {groupTitle(g)} — вместе <b>{fmtBytes(g.reclaim)}</b>
      </p>
      <p className="reason dim small">{g.items[0].reasons[0]}</p>
      <div className="card-foot">
        <button className="btn-link" onClick={() => onToggle(g.key)} aria-expanded={expanded}>
          {expanded ? "Скрыть процессы" : "Показать процессы"}
        </button>
        <PolicyLine policy={g.policy} />
        <span className="spacer" />
        {killable && (
          <button
            className={g.policy.kind === "confirm" ? "btn btn-kill btn-warn" : "btn btn-kill"}
            onClick={() => killGroup(g, onKill)}
          >
            Снять все
          </button>
        )}
      </div>
      {expanded && (
        <div className="card-procs">
          {g.items.map((it) => (
            <div key={it.key} className="card-proc" onClick={() => onOpen(it)}>
              <span className="dim">{it.pid}</span>
              <span className="ellipsis">{it.reasons[0]}</span>
              <b>{fmtBytes(it.reclaim)}</b>
              <KillButton target={targetOf(it)} onKill={onKill} />
            </div>
          ))}
        </div>
      )}
    </article>
  );
});

function PolicyLine({ policy }: { policy: KillPolicy }) {
  if (policy.kind === "protected")
    return (
      <span className="pol-protected small">
        <LockIcon /> {policy.reason}
      </span>
    );
  return (
    <span className={`pol-${policy.kind} small`} title={policy.kind === "confirm" ? policy.reason : undefined}>
      {POLICY_LABEL[policy.kind]}
    </span>
  );
}
