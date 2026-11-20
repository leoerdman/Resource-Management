import { useEffect, useMemo, useState } from "react";
import { backend } from "../api";
import { errText } from "../format";
import type { Settings } from "../types";
import { NumField, Switch } from "./Controls";

interface Props {
  onToast: (ok: boolean, text: string) => void;
  onOpenGuard: () => void;
}

/** Every number in the form, to refuse saving half-typed values before the backend sees them. */
function hasBadNumber(s: Settings): boolean {
  const t = s.thresholds;
  const nums = [t.hog_mb, t.hog_pct, t.growing_mb_per_min, t.idle_mb, t.idle_age_min, t.duplicate_count];
  return nums.some((n) => !Number.isFinite(n) || n < 0);
}

export function SettingsTab({ onToast, onOpenGuard }: Props) {
  const [saved, setSaved] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let alive = true;
    backend()
      .then((b) => b.getSettings())
      .then((s) => {
        if (!alive) return;
        setSaved(s);
        setDraft(structuredClone(s));
      })
      .catch((e) => alive && setLoadError(errText(e)));
    return () => {
      alive = false;
    };
  }, []);

  const dirty = useMemo(() => saved != null && draft != null && JSON.stringify(saved) !== JSON.stringify(draft), [saved, draft]);

  if (!draft || !saved) {
    return <div className="empty">{loadError ? <span className="error">Настройки недоступны: {loadError}</span> : "Загрузка…"}</div>;
  }

  // Small typed updaters keep the JSX readable.
  const setApp = <K extends keyof Settings["app"]>(k: K, v: Settings["app"][K]) =>
    setDraft((d) => d && { ...d, app: { ...d.app, [k]: v } });
  const setThr = <K extends keyof Settings["thresholds"]>(k: K, v: Settings["thresholds"][K]) =>
    setDraft((d) => d && { ...d, thresholds: { ...d.thresholds, [k]: v } });

  async function save() {
    if (!draft) return;
    if (hasBadNumber(draft)) {
      setSaveError("Заполните все числовые поля неотрицательными числами");
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      // Сторож правится во вкладке «Сторож» и из трея — его берём из файла, а не из черновика.
      const b = await backend();
      const fresh = await b.getSettings();
      const s = await b.saveSettings({ ...draft, guard: fresh.guard });
      setSaved(s);
      setDraft(structuredClone(s));
      onToast(true, "Сохранено, применено сразу");
    } catch (e) {
      setSaveError(errText(e));
    } finally {
      setSaving(false);
    }
  }

  async function openConfig() {
    try {
      await (await backend()).openConfig();
    } catch (e) {
      onToast(false, errText(e));
    }
  }

  const t = draft.thresholds;

  return (
    <div className="pane-scroll">
      <form
        className="settings"
        onSubmit={(e) => {
          e.preventDefault();
          if (dirty && !saving) save();
        }}
      >
        <fieldset>
          <legend>Приложение</legend>
          <Switch label="Запускать при входе в macOS" checked={draft.app.autostart} onChange={(v) => setApp("autostart", v)} />
          <Switch
            label="Показывать занятую память рядом со значком"
            hint="в строке меню, например «14,2 ГБ»"
            checked={draft.app.tray_title}
            onChange={(v) => setApp("tray_title", v)}
          />
        </fieldset>

        <fieldset>
          <legend>Сторож памяти</legend>
          <div className="settings-note">
            <span className="dim">Режим, красная зона, жертвы, пределы и уведомления — во вкладке «Сторож».</span>
            <button type="button" className="btn btn-small" onClick={onOpenGuard}>
              Открыть «Сторож»
            </button>
          </div>
        </fieldset>

        <fieldset>
          <legend>Подозреваемые</legend>
          <div className="fields">
            <NumField label="Большой от" unit="МБ" value={t.hog_mb} onChange={(v) => setThr("hog_mb", v)} />
            <NumField label="…или от" unit="% ОЗУ" value={t.hog_pct} onChange={(v) => setThr("hog_pct", v)} />
            <NumField label="Растёт от" unit="МБ/мин" value={t.growing_mb_per_min} onChange={(v) => setThr("growing_mb_per_min", v)} />
            <NumField label="Простаивает от" unit="МБ" value={t.idle_mb} onChange={(v) => setThr("idle_mb", v)} />
            <NumField label="…и дольше" unit="мин" value={t.idle_age_min} onChange={(v) => setThr("idle_age_min", v)} />
            <NumField label="Брошенных копий от" unit="шт" value={t.duplicate_count} onChange={(v) => setThr("duplicate_count", v)} />
          </div>
        </fieldset>

        {saveError && <div className="error save-error">{saveError}</div>}
        <div className="settings-actions">
          <button type="submit" className="btn btn-primary" disabled={!dirty || saving}>
            {saving ? "Сохраняю…" : "Сохранить"}
          </button>
          <button
            type="button"
            className="btn"
            disabled={!dirty || saving}
            onClick={() => {
              setDraft(structuredClone(saved));
              setSaveError(null);
            }}
          >
            Отменить
          </button>
          <span className="spacer" />
          <button type="button" className="btn" onClick={openConfig}>
            Открыть config.toml
          </button>
        </div>
        <div className="dim small mono">{saved.config_file}</div>
      </form>
    </div>
  );
}
