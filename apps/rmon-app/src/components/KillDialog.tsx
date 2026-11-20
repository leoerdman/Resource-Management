import { useEffect, useRef, useState } from "react";
import { backend } from "../api";
import { clause, errText, fmtBytes } from "../format";
import type { KillTarget } from "../types";

export interface KillResult {
  ok: boolean;
  text: string;
}

interface Props {
  target: KillTarget;
  onCancel: () => void;
  onResult: (r: KillResult) => void;
}

export function KillDialog({ target, onCancel, onResult }: Props) {
  const [busy, setBusy] = useState(false);
  const needsConfirm = target.policy.kind === "confirm";
  const cancelRef = useRef<HTMLButtonElement>(null);
  const killRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    // Risky kills focus «Отмена» so a stray Enter does nothing harmful.
    (needsConfirm ? cancelRef : killRef).current?.focus();
  }, [needsConfirm]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onCancel]);

  async function run() {
    setBusy(true);
    try {
      const b = await backend();
      const out = await b.kill({
        pid: target.pid,
        startedAt: target.started_at,
        confirm: needsConfirm,
        reason: target.reason,
      });
      onResult({ ok: true, text: out.dry_run ? `Пробный режим: ${out.message}` : out.message });
    } catch (e) {
      onResult({ ok: false, text: errText(e) });
    }
  }

  return (
    <div className="overlay" onMouseDown={() => !busy && onCancel()}>
      <div
        className={needsConfirm ? "dialog dialog-warn" : "dialog"}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="kill-title"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <h2 id="kill-title">
          Снять {target.name} <span className="dim">({target.pid})</span>?
        </h2>
        {needsConfirm && target.policy.kind === "confirm" && (
          <div className="warn-box">
            Это процесс приложения <b>«{target.app_group}»</b>. {clause(target.policy.reason)}.
          </div>
        )}
        <p className="dim">
          Сначала SIGTERM, через несколько секунд — SIGKILL, если процесс не завершится.
          {target.reclaim > 0 && <> Освободит около {fmtBytes(target.reclaim)}.</>}
        </p>
        <div className="dialog-actions">
          <button ref={cancelRef} className="btn" onClick={onCancel} disabled={busy}>
            Отмена
          </button>
          <button ref={killRef} className="btn btn-danger" onClick={run} disabled={busy}>
            {busy ? "Снимаю…" : needsConfirm ? "Всё равно снять" : "Снять"}
          </button>
        </div>
      </div>
    </div>
  );
}
