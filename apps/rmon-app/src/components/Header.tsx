import { memo } from "react";
import { fmtBytes, PRESSURE_LABEL } from "../format";
import type { HistoryPoint, SystemMemory } from "../types";
import { HistoryChart } from "./HistoryChart";

interface Props {
  system: SystemMemory;
  history: HistoryPoint[];
  takenAt: number;
}

export const Header = memo(function Header({ system: s, history, takenAt }: Props) {
  return (
    <header className="header">
      <div className="summary">
        <div className="stats">
          <span className={`pressure p-${s.pressure}`} title={`Доступно ${s.available_pct} % памяти`}>
            <span className="dot" />
            Давление {PRESSURE_LABEL[s.pressure]}
          </span>
          <span>
            Занято <b>{fmtBytes(s.used)}</b> из {fmtBytes(s.total)}
          </span>
          <span title={s.swap_total ? `Своп ${fmtBytes(s.swap_used)} из ${fmtBytes(s.swap_total)}` : undefined}>
            Своп <b>{fmtBytes(s.swap_used)}</b>
          </span>
          <span>
            Сжато <b>{fmtBytes(s.compressed)}</b>
          </span>
        </div>
        <MemoryBar s={s} />
      </div>
      <HistoryChart history={history} total={s.total} now={takenAt} />
    </header>
  );
});

const SEGMENTS = [
  { key: "app", label: "Приложения", cls: "seg-app" },
  { key: "wired", label: "Wired", cls: "seg-wired" },
  { key: "compressed", label: "Сжато", cls: "seg-compressed" },
  { key: "other", label: "Прочее", cls: "seg-other" },
  { key: "cached_files", label: "Кэш", cls: "seg-cached" },
  { key: "free", label: "Свободно", cls: "seg-free" },
] as const;

function MemoryBar({ s }: { s: SystemMemory }) {
  // Сегменты складываются в total (ядро гарантирует used + cached + free = total и
  // used = app + wired + compressed + other); нормируем по сумме на случай округлений.
  const sum = SEGMENTS.reduce((a, g) => a + Math.max(0, s[g.key]), 0) || 1;
  return (
    <div className="membar-wrap">
      <div className="membar" role="img" aria-label="Разбивка памяти">
        {SEGMENTS.map((g) => (
          <div
            key={g.key}
            className={`seg ${g.cls}`}
            style={{ width: `${(Math.max(0, s[g.key]) / sum) * 100}%` }}
            title={`${g.label}: ${fmtBytes(s[g.key])}`}
          />
        ))}
      </div>
      <div className="legend">
        {SEGMENTS.map((g) => (
          <span key={g.key}>
            <i className={`sw ${g.cls}`} />
            {g.label} <b>{fmtBytes(s[g.key])}</b>
          </span>
        ))}
      </div>
    </div>
  );
}
