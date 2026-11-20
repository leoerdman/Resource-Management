import { memo, useEffect, useMemo, useState } from "react";
import { backend } from "../api";
import { errText, fmtAge, fmtBytes, fmtGrowth, fmtPct, plural, STATE_LABEL } from "../format";
import type { ProcBrief, ProcessDetails, ProcKey } from "../types";
import { KillButton, PolicyTag } from "./Kill";
import type { KillRequest } from "./KillDialog";
import { processTarget } from "./ProcessesTab";

const REFRESH_MS = 5000;
const MAX_CHILDREN = 60;

interface Props {
  proc: ProcKey;
  /** A modal is on top: Esc belongs to it. */
  blocked: boolean;
  onClose: () => void;
  onOpen: (k: ProcKey) => void;
  onKill: (r: KillRequest) => void;
  onToast: (ok: boolean, text: string) => void;
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // WebViews without clipboard permission: fall back to a hidden textarea.
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    if (!ok) throw new Error("буфер обмена недоступен");
  }
}

export function DetailsDrawer({ proc, blocked, onClose, onOpen, onKill, onToast }: Props) {
  const [data, setData] = useState<ProcessDetails | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Load now and every 5 s while open; a new process resets the panel.
  useEffect(() => {
    let alive = true;
    setData(null);
    setError(null);
    const load = async () => {
      try {
        const b = await backend();
        const d = await b.details(proc.pid, proc.started_at);
        if (alive) {
          setData(d);
          setError(null);
        }
      } catch (e) {
        if (alive) setError(errText(e));
      }
    };
    load();
    const id = setInterval(load, REFRESH_MS);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [proc.pid, proc.started_at]);

  useEffect(() => {
    if (blocked) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [blocked, onClose]);

  const copy = (done: string, text: string) =>
    copyText(text).then(
      () => onToast(true, done),
      (e) => onToast(false, `Не удалось скопировать: ${errText(e)}`),
    );

  const r = data?.row;
  const now = Date.now() / 1000;

  return (
    <div className="drawer-backdrop" onMouseDown={onClose}>
      <aside className="drawer" role="dialog" aria-label="Подробности процесса" onMouseDown={(e) => e.stopPropagation()}>
        <div className="drawer-head">
          <div className="drawer-title ellipsis">
            {r?.name ?? "…"} <span className="dim">{proc.pid}</span>
          </div>
          <button className="icon-btn" onClick={onClose} title="Закрыть (Esc)" aria-label="Закрыть">
            ×
          </button>
        </div>

        {error && <div className={data ? "drawer-note" : "drawer-note error"}>{data ? `Не обновилось: ${error}` : error}</div>}
        {!data && !error && <div className="dim pad">Загрузка…</div>}

        {data && r && (
          <div className={error ? "drawer-body stale" : "drawer-body"}>
            <div className="dim">
              {r.app_group}
              {r.app_group !== r.name ? ` · ${r.name}` : ""} · {STATE_LABEL[r.state]} · uid {r.uid}
            </div>

            <section>
              <h3>Рабочий каталог</h3>
              {data.cwd ? (
                <div className="cwd-line">
                  <span className="cwd" title={data.cwd}>
                    {data.cwd}
                  </span>
                  <button className="btn btn-small" onClick={() => copy("Каталог скопирован", data.cwd!)}>
                    Копировать
                  </button>
                </div>
              ) : (
                <div className="dim">недоступен (чужой или системный процесс)</div>
              )}
            </section>

            <section>
              <div className="section-head">
                <h3>Строка запуска</h3>
                <button className="btn btn-small" onClick={() => copy("Строка запуска скопирована", data.command_line)}>
                  Копировать
                </button>
              </div>
              <pre className="cmdline">{data.command_line || "—"}</pre>
            </section>

            <section className="facts">
              <Fact label="Память" value={fmtBytes(r.footprint ?? r.rss)} strong />
              <Fact label="RSS" value={fmtBytes(r.rss)} />
              <Fact label="Рост" value={r.growth_per_min == null ? "—" : `${fmtGrowth(r.growth_per_min)} МБ/мин`} />
              <Fact label="CPU" value={`${fmtPct(r.cpu_pct)} %`} />
              <Fact label="Потоки" value={String(r.threads)} />
              <Fact label="Возраст" value={fmtAge(r.started_at, now)} />
            </section>

            <section>
              <h3>Память за последние минуты</h3>
              <Sparkline series={data.series} />
            </section>

            <section className="policy-line">
              <PolicyTag policy={r.policy} />
              {r.policy.kind !== "free" && <span className="dim small">{r.policy.reason}</span>}
              <span className="spacer" />
              <KillButton target={processTarget(r, "Подробности")} onKill={onKill} />
            </section>

            <section>
              <h3>Родители</h3>
              {data.ancestors.length ? (
                <ol className="chain">
                  {data.ancestors.map((a) => (
                    <BriefRow key={a.pid} b={a} onOpen={onOpen} />
                  ))}
                </ol>
              ) : (
                <div className="dim">нет</div>
              )}
            </section>

            <section>
              <h3>
                Потомки:{" "}
                {data.descendants.length ? (
                  <>
                    {data.descendants.length} {plural(data.descendants.length, "процесс", "процесса", "процессов")},{" "}
                    {fmtBytes(data.descendants_total)}
                  </>
                ) : (
                  "нет"
                )}
              </h3>
              {data.descendants.length > 0 && (
                <ul className="chain">
                  {data.descendants.slice(0, MAX_CHILDREN).map((d) => (
                    <BriefRow key={d.pid} b={d} onOpen={onOpen} />
                  ))}
                  {data.descendants.length > MAX_CHILDREN && (
                    <li className="dim small">ещё {data.descendants.length - MAX_CHILDREN}…</li>
                  )}
                </ul>
              )}
            </section>
          </div>
        )}
      </aside>
    </div>
  );
}

function Fact({ label, value, strong }: { label: string; value: string; strong?: boolean }) {
  return (
    <div className="fact">
      <div className="dim small">{label}</div>
      <div className={strong ? "strong" : undefined}>{value}</div>
    </div>
  );
}

function BriefRow({ b, onOpen }: { b: ProcBrief; onOpen: (k: ProcKey) => void }) {
  return (
    <li>
      <button className="brief" onClick={() => onOpen(b)} title={b.cmd}>
        <span className="dim">{b.pid}</span>
        <b>{b.name}</b>
        <span className="dim ellipsis">{b.cmd}</span>
        <span className="num">{fmtBytes(b.reclaim)}</span>
      </button>
    </li>
  );
}

const SW = 400;
const SH = 64;

/** Memory over time from `ProcessDetails.series`, with min/max labels. */
const Sparkline = memo(function Sparkline({ series }: { series: [number, number][] }) {
  const geo = useMemo(() => {
    if (series.length < 2) return null;
    const t0 = series[0][0];
    const t1 = series[series.length - 1][0];
    let min = Infinity;
    let max = -Infinity;
    for (const [, v] of series) {
      min = Math.min(min, v);
      max = Math.max(max, v);
    }
    const span = Math.max(1, t1 - t0);
    // Pad the range so a flat line sits mid-height instead of glued to an edge.
    const pad = Math.max((max - min) * 0.12, max * 0.02, 1);
    const lo = Math.max(0, min - pad);
    const hi = max + pad;
    const x = (t: number) => ((t - t0) / span) * SW;
    const y = (v: number) => SH - ((v - lo) / (hi - lo)) * SH;
    let d = "";
    series.forEach(([t, v], i) => {
      d += `${i ? "L" : "M"}${x(t).toFixed(1)} ${y(v).toFixed(1)}`;
    });
    const area = `${d}L${SW} ${SH}L0 ${SH}Z`;
    return { d, area, min, max, minutes: Math.round(span / 60) };
  }, [series]);

  if (!geo) return <div className="dim small">Пока мало данных — история копится с запуска rmon.</div>;
  return (
    <div className="spark">
      <svg viewBox={`0 0 ${SW} ${SH}`} preserveAspectRatio="none" aria-label="График памяти процесса">
        <path d={geo.area} className="spark-area" />
        <path d={geo.d} className="line line-used" />
      </svg>
      <span className="axis axis-top">макс {fmtBytes(geo.max)}</span>
      <span className="axis axis-left">мин {fmtBytes(geo.min)}</span>
      <span className="axis axis-right">за {geo.minutes || "<1"} мин</span>
    </div>
  );
});
