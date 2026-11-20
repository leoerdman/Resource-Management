import { useEffect, useRef, useState, type ReactNode } from "react";

interface Props {
  title: ReactNode;
  children: ReactNode;
  confirmLabel: string;
  busyLabel?: string;
  danger?: boolean;
  /** Resolve to close; reject to keep the dialog open (the caller shows the error). */
  onConfirm: () => Promise<void>;
  onCancel: () => void;
}

/** Generic yes/no dialog for reversible but noticeable actions. */
export function ConfirmDialog({ title, children, confirmLabel, busyLabel, danger, onConfirm, onCancel }: Props) {
  const [busy, setBusy] = useState(false);
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => cancelRef.current?.focus(), []);
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
    setBusy(true);
    try {
      await onConfirm();
    } catch {
      setBusy(false);
    }
  }

  return (
    <div className="overlay" onMouseDown={() => !busy && onCancel()}>
      <div className="dialog" role="alertdialog" aria-modal="true" onMouseDown={(e) => e.stopPropagation()}>
        <h2>{title}</h2>
        <div className="dialog-text">{children}</div>
        <div className="dialog-actions">
          <button ref={cancelRef} className="btn" onClick={onCancel} disabled={busy}>
            Отмена
          </button>
          <button className={danger ? "btn btn-danger" : "btn btn-primary"} onClick={run} disabled={busy}>
            {busy ? busyLabel ?? "Выполняю…" : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
