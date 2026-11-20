import { memo, useMemo, useRef, useState, type MouseEvent } from "react";
import { fmtBytes, fmtClock, PRESSURE_LABEL } from "../format";
import type { HistoryPoint, Pressure } from "../types";

const WINDOW_S = 15 * 60;
const W = 1000;
const H = 100;

interface Props {
  history: HistoryPoint[];
  total: number;
  now: number;
}

interface Band {
  x: number;
  w: number;
  p: Pressure;
}

/** Hand-written SVG chart: used + swap over the last 15 min, pressure as background bands. */
export const HistoryChart = memo(function HistoryChart({ history, total, now }: Props) {
  const [hover, setHover] = useState<number | null>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const t0 = now - WINDOW_S;

  const geo = useMemo(() => {
    let max = total;
    for (const p of history) max = Math.max(max, p.used, p.swap_used);
    const x = (t: number) => ((t - t0) / WINDOW_S) * W;
    const y = (v: number) => H - (v / max) * H;
    let used = "";
    let swap = "";
    for (let i = 0; i < history.length; i++) {
      const p = history[i];
      const cmd = i ? "L" : "M";
      const xs = x(p.t).toFixed(1);
      used += `${cmd}${xs} ${y(p.used).toFixed(1)}`;
      swap += `${cmd}${xs} ${y(p.swap_used).toFixed(1)}`;
    }
    // Merge consecutive non-normal points into bands; each point covers until the next one.
    const bands: Band[] = [];
    for (let i = 0; i < history.length; i++) {
      const p = history[i];
      if (p.pressure === "normal") continue;
      const x1 = x(p.t);
      const x2 = i + 1 < history.length ? x(history[i + 1].t) : x(now);
      const last = bands[bands.length - 1];
      if (last && last.p === p.pressure && Math.abs(last.x + last.w - x1) < 0.01) last.w = x2 - last.x;
      else bands.push({ x: x1, w: x2 - x1, p: p.pressure });
    }
    return { max, used, swap, bands, x };
  }, [history, total, t0, now]);

  const onMove = (e: MouseEvent) => {
    const box = boxRef.current;
    if (!box || !history.length) return;
    const r = box.getBoundingClientRect();
    const t = t0 + ((e.clientX - r.left) / r.width) * WINDOW_S;
    // Binary search for the nearest point by time.
    let lo = 0;
    let hi = history.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (history[mid].t < t) lo = mid + 1;
      else hi = mid;
    }
    if (lo > 0 && Math.abs(history[lo - 1].t - t) < Math.abs(history[lo].t - t)) lo--;
    setHover(lo);
  };

  const hp = hover != null ? history[hover] : null;
  const hx = hp ? (geo.x(hp.t) / W) * 100 : 0;

  return (
    <div className="chart">
      <div className="chart-box" ref={boxRef} onMouseMove={onMove} onMouseLeave={() => setHover(null)}>
        <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" aria-label="История памяти за 15 минут">
          {geo.bands.map((b, i) => (
            <rect key={i} x={b.x} y={0} width={b.w} height={H} className={`band band-${b.p}`} />
          ))}
          {[0.25, 0.5, 0.75].map((f) => (
            <line key={f} x1={0} x2={W} y1={H * f} y2={H * f} className="grid" />
          ))}
          <path d={geo.swap} className="line line-swap" />
          <path d={geo.used} className="line line-used" />
        </svg>
        <span className="axis axis-top">{fmtBytes(geo.max)}</span>
        <span className="axis axis-mid">{fmtBytes(geo.max / 2)}</span>
        <span className="axis axis-left">15 мин назад</span>
        <span className="axis axis-right">сейчас</span>
        {hp && (
          <>
            <div className="cursor" style={{ left: `${hx}%` }} />
            <div className={hx > 60 ? "tip tip-left" : "tip"} style={{ left: `${hx}%` }}>
              <div>
                {fmtClock(hp.t)} · <span className={`p-${hp.pressure}`}>{PRESSURE_LABEL[hp.pressure]}</span>
              </div>
              <div>
                <i className="sw sw-used" />
                Занято {fmtBytes(hp.used)}
              </div>
              <div>
                <i className="sw sw-swap" />
                Своп {fmtBytes(hp.swap_used)} · сжато {fmtBytes(hp.compressed)}
              </div>
            </div>
          </>
        )}
      </div>
      <div className="legend chart-legend">
        <span>
          <i className="sw sw-used" />
          Занято
        </span>
        <span>
          <i className="sw sw-swap" />
          Своп
        </span>
        <span>
          <i className="sw band-warning" />
          повышенное
        </span>
        <span>
          <i className="sw band-critical" />
          критическое
        </span>
      </div>
    </div>
  );
});
