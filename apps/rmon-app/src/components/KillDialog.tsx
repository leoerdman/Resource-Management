import { useEffect, useRef, useState } from "react";
import { backend } from "../api";
import { clause, errText, fmtBytes, plural } from "../format";
import type { KillTarget, ProcKey } from "../types";

/** One process, or a batch («Снять все» for a group of abandoned copies). */
export interface KillRequest {
  targets: KillTarget[];
  /** Title for a batch, e.g. «6 брошенных копий «mcp-memory-bank»». */
  title?: string;
}

export interface KillResult {
  ok: boolean;
  text: string;
  killed: ProcKey[];
}

interface Props {
  req: KillRequest;
  onCancel: () => void;
  onResult: (r: KillResult) => void;
}

export function KillDialog({ req, onCancel, onResult }: Props) {
  const { targets } = req;
  const single = targets.length === 1 ? targets[0] : null;
  const [progress, setProgress] = useState<number | null>(null);
  const busy = progress != null;
  const risky = targets.filter((t) => t.policy.kind === "confirm");
  const reclaim = targets.reduce((a, t) => a + t.reclaim, 0);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const killRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    // Risky kills focus «Отмена» so a stray Enter does nothing harmful.
    (risky.length ? cancelRef : killRef).current?.focus();
  }, [risky.length]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) {
        e.stopPropagation();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [busy, onCancel]);

  async function run() {
    const b = await backend();
    // One by one: each kill waits for SIGTERM grace, and the backend re-checks identity each time.
    let done = 0;
    let freed = 0;
    let lastMsg = "";
    let lastErr = "";
    const killed: ProcKey[] = [];
    for (let i = 0; i < targets.length; i++) {
      const t = targets[i];
      setProgress(i);
      try {
        const out = await b.kill({
          pid: t.pid,
          startedAt: t.started_at,
          confirm: t.policy.kind === "confirm",
          reason: t.reason,
        });
        lastMsg = out.dry_run ? `Пробный режим: ${out.message}` : out.message;
        if (out.exited) {
          done++;
          freed += out.reclaimed;
          killed.push({ pid: t.pid, started_at: t.started_at });
        }
      } catch (e) {
        lastErr = errText(e);
      }
    }
    if (single) {
      onResult(lastErr ? { ok: false, text: lastErr, killed } : { ok: true, text: lastMsg, killed });
      return;
    }
    const text =
      `Снято ${done} из ${targets.length}, освобождено ${fmtBytes(freed)}` + (lastErr ? `. Последняя ошибка: ${lastErr}` : "");
    onResult({ ok: done > 0, text, killed });
  }

  const apps = [...new Set(risky.map((t) => t.app_group))];

  return (
    <div className="overlay" onMouseDown={() => !busy && onCancel()}>
      <div
        className={risky.length ? "dialog dialog-warn" : "dialog"}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="kill-title"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <h2 id="kill-title">
          {single ? (
            <>
              Снять {single.name} <span className="dim">({single.pid})</span>?
            </>
          ) : (
            <>Снять все: {req.title ?? `${targets.length} ${plural(targets.length, "процесс", "процесса", "процессов")}`}?</>
          )}
        </h2>
        {single && single.policy.kind === "confirm" && (
          <div className="warn-box">
            Это процесс приложения <b>«{single.app_group}»</b>. {clause(single.policy.reason)}.
          </div>
        )}
        {!single && risky.length > 0 && risky[0].policy.kind === "confirm" && (
          <div className="warn-box">
            {risky.length} из {targets.length} — процессы {apps.length > 1 ? "приложений" : "приложения"}{" "}
            <b>{apps.map((a) => `«${a}»`).join(", ")}</b>. {clause(risky[0].policy.reason)}.
          </div>
        )}
        {!single && (
          <ul className="kill-list">
            {targets.map((t, i) => (
              <li key={t.pid} className={progress != null && i < progress ? "dim" : undefined}>
                <span>
                  {t.name} <span className="dim">{t.pid}</span>
                </span>
                <span className="dim">{fmtBytes(t.reclaim)}</span>
              </li>
            ))}
          </ul>
        )}
        <p className="dim">
          {single ? "Сначала SIGTERM" : "По одному: сначала SIGTERM"}, через несколько секунд — SIGKILL, если процесс не
          завершится.
          {reclaim > 0 && <> Освободит около {fmtBytes(reclaim)}.</>}
        </p>
        <div className="dialog-actions">
          <button ref={cancelRef} className="btn" onClick={onCancel} disabled={busy}>
            Отмена
          </button>
          <button ref={killRef} className="btn btn-danger" onClick={run} disabled={busy}>
            {busy
              ? single
                ? "Снимаю…"
                : `Снимаю ${progress + 1} из ${targets.length}…`
              : risky.length
                ? single
                  ? "Всё равно снять"
                  : "Всё равно снять все"
                : single
                  ? "Снять"
                  : `Снять все (${targets.length})`}
          </button>
        </div>
      </div>
    </div>
  );
}
