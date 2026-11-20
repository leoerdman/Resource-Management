export interface Toast {
  id: number;
  ok: boolean;
  text: string;
}

export function Toasts({ toasts, onDismiss }: { toasts: Toast[]; onDismiss: (id: number) => void }) {
  if (!toasts.length) return null;
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={t.ok ? "toast ok" : "toast err"} onClick={() => onDismiss(t.id)} title="Скрыть">
          {t.text}
        </div>
      ))}
    </div>
  );
}
