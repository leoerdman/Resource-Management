import { useCallback, useEffect, useState } from "react";
import { backend } from "../api";
import { errText, fmtDateTime } from "../format";
import type { JournalEntry } from "../types";

export function JournalTab() {
  const [entries, setEntries] = useState<JournalEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [openIdx, setOpenIdx] = useState<number | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const b = await backend();
      setEntries(await b.readJournal());
      setOpenIdx(null);
    } catch (e) {
      setError(errText(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  return (
    <div className="pane-scroll">
      <div className="toolbar">
        <button className="btn" onClick={load} disabled={loading}>
          {loading ? "Обновляю…" : "Обновить"}
        </button>
        <span className="dim small">Действия rmon и события сторожа, новые сверху. Нажмите на строку — исходная запись.</span>
      </div>
      {error && <div className="error pad">Не удалось прочитать журнал: {error}</div>}
      {entries && !entries.length && <div className="empty">Журнал пуст</div>}
      {entries && (
        <ul className="journal">
          {entries.map((e, i) => (
            <li key={i} onClick={() => setOpenIdx(openIdx === i ? null : i)}>
              <div className="jline">
                <span className="jtime dim">{e.ts != null ? fmtDateTime(e.ts) : "—"}</span>
                <span className={`jsrc src-${e.source}`}>{e.source}</span>
                <span className="jtext">{e.text}</span>
              </div>
              {openIdx === i && <pre className="jraw">{JSON.stringify(e.raw, null, 2)}</pre>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
