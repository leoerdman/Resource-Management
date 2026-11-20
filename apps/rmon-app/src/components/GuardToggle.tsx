import { errText } from "../format";
import { guardStore, modeBrief, useGuard } from "../guardStore";

/**
 * Сторож в полосе вкладок, справа: точка режима, подпись и переключатель «вкл/выкл».
 * Подпись открывает вкладку «Сторож»; переключатель пишет `[guard] enabled` в config.toml.
 */
export function GuardToggle({ onOpen, onToast }: { onOpen: () => void; onToast: (ok: boolean, text: string) => void }) {
  const { status, settings, saving } = useGuard();
  const enabled = settings?.enabled ?? status?.config.enabled ?? false;
  const { label, tone } = modeBrief(status, settings);
  const detail =
    status && (status.mode.kind === "standby" || status.mode.kind === "broken") ? status.mode.detail : undefined;

  async function toggle(v: boolean) {
    try {
      await guardStore.patch({ enabled: v });
      onToast(true, v ? "Сторож включён" : "Сторож выключен");
    } catch (e) {
      onToast(false, errText(e));
    }
  }

  return (
    <div className="guard-toggle">
      <button className="guard-toggle-label" onClick={onOpen} title={detail ?? "Открыть вкладку «Сторож»"}>
        <span className={`guard-dot tone-${tone}`} aria-hidden="true" />
        Сторож <span className="dim">· {label}</span>
      </button>
      <input
        type="checkbox"
        role="switch"
        className="switch small"
        aria-label="Сторож памяти включён"
        checked={enabled}
        disabled={saving || !status}
        onChange={(e) => void toggle(e.target.checked)}
      />
    </div>
  );
}
