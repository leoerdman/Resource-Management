import { useEffect, useMemo, useState } from "react";
import { backend } from "../api";
import { errText } from "../format";
import type { GuardLimit, Settings } from "../types";
import { NumField, SelectField, Switch } from "./Controls";
import { fmtSecs } from "./GuardTab";

interface Props {
  onToast: (ok: boolean, text: string) => void;
}

const LEVELS_RED = [
  { value: 2, label: "жёлтое (2)" },
  { value: 4, label: "красное (4)" },
];
const LEVELS_ALL = [{ value: 1, label: "нормальное (1)" }, ...LEVELS_RED];

/** Every number in the form, to refuse saving half-typed values before the backend sees them. */
function hasBadNumber(s: Settings): boolean {
  const g = s.guard;
  const t = s.thresholds;
  const nums = [
    g.sustain_secs, g.after_kill_secs, g.term_grace_secs, g.min_victim_mb,
    g.red.min_available_pct, g.red.swap_red_mb,
    t.hog_mb, t.hog_pct, t.growing_mb_per_min, t.idle_mb, t.idle_age_min, t.duplicate_count,
    ...g.limits.flatMap((l) => [l.max, l.window_secs]),
  ];
  return nums.some((n) => !Number.isFinite(n) || n < 0);
}

export function SettingsTab({ onToast }: Props) {
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
  const setGuard = <K extends keyof Settings["guard"]>(k: K, v: Settings["guard"][K]) =>
    setDraft((d) => d && { ...d, guard: { ...d.guard, [k]: v } });
  const setRed = <K extends keyof Settings["guard"]["red"]>(k: K, v: Settings["guard"]["red"][K]) =>
    setDraft((d) => d && { ...d, guard: { ...d.guard, red: { ...d.guard.red, [k]: v } } });
  const setThr = <K extends keyof Settings["thresholds"]>(k: K, v: Settings["thresholds"][K]) =>
    setDraft((d) => d && { ...d, thresholds: { ...d.thresholds, [k]: v } });
  const setLimits = (f: (ls: GuardLimit[]) => GuardLimit[]) =>
    setDraft((d) => d && { ...d, guard: { ...d.guard, limits: f(d.guard.limits) } });

  async function save() {
    if (!draft) return;
    if (hasBadNumber(draft)) {
      setSaveError("Заполните все числовые поля неотрицательными числами");
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      const s = await (await backend()).saveSettings(draft);
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

  const g = draft.guard;
  const t = draft.thresholds;
  const swapGb = Math.round((g.red.swap_red_mb / 1024) * 10) / 10;

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
          <Switch label="Сторож включён" checked={g.enabled} onChange={(v) => setGuard("enabled", v)} />
          <Switch label="Режим пробы" hint="только пишет, кого снял бы" checked={g.dry_run} onChange={(v) => setGuard("dry_run", v)} />
          <Switch label="Уведомлять о снятии" checked={g.notify} onChange={(v) => setGuard("notify", v)} />
          <Switch label="Уведомлять в режиме пробы" checked={g.notify_dry_run} onChange={(v) => setGuard("notify_dry_run", v)} />

          <div className="fields">
            <NumField label="Красное держится" unit="с" value={g.sustain_secs} onChange={(v) => setGuard("sustain_secs", v)} hint="прежде чем снимать" />
            <NumField label="Пауза после снятия" unit="с" value={g.after_kill_secs} onChange={(v) => setGuard("after_kill_secs", v)} />
            <NumField label="Ожидание после SIGTERM" unit="с" value={g.term_grace_secs} onChange={(v) => setGuard("term_grace_secs", v)} hint="потом SIGKILL" />
            <NumField label="Жертва от" unit="МБ" value={g.min_victim_mb} onChange={(v) => setGuard("min_victim_mb", v)} />
          </div>

          <h4>Красная зона — любое из условий</h4>
          <div className="fields">
            <SelectField label="Давление от" value={g.red.pressure_level} options={LEVELS_RED} onChange={(v) => setRed("pressure_level", v)} />
            <NumField label="Доступно не больше" unit="%" value={g.red.min_available_pct} onChange={(v) => setRed("min_available_pct", v)} />
            <NumField
              label="Своп от"
              unit="ГБ"
              value={swapGb}
              onChange={(v) => setRed("swap_red_mb", Number.isNaN(v) ? NaN : Math.round(v * 1024))}
            />
            <SelectField
              label="…при давлении от"
              value={g.red.swap_red_from_level}
              options={LEVELS_ALL}
              onChange={(v) => setRed("swap_red_from_level", v)}
            />
          </div>

          <h4>Пределы снятий</h4>
          <div className="limits">
            {g.limits.map((l, i) => (
              <div key={i} className="limit-row">
                <span>не больше</span>
                <NumField
                  label=""
                  value={l.max}
                  onChange={(v) => setLimits((ls) => ls.map((x, j) => (j === i ? { ...x, max: v } : x)))}
                />
                <span>за</span>
                <NumField
                  label=""
                  unit="с"
                  value={l.window_secs}
                  onChange={(v) => setLimits((ls) => ls.map((x, j) => (j === i ? { ...x, window_secs: v } : x)))}
                />
                <span className="dim small">{Number.isFinite(l.window_secs) && l.window_secs > 0 ? `= ${fmtSecs(l.window_secs)}` : ""}</span>
                <button
                  type="button"
                  className="icon-btn"
                  title="Убрать предел"
                  aria-label="Убрать предел"
                  onClick={() => setLimits((ls) => ls.filter((_, j) => j !== i))}
                >
                  ×
                </button>
              </div>
            ))}
            <button type="button" className="btn-link" onClick={() => setLimits((ls) => [...ls, { window_secs: 3600, max: 5 }])}>
              + Добавить предел
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
