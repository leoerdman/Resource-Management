import { errText, fmtBytes } from "../format";
import { guardStore } from "../guardStore";
import type { GuardConfig, GuardLimit, GuardSettings } from "../types";
import { NumField, SelectField, Switch } from "./Controls";

const MB = 1024 * 1024;

const LEVEL_LABEL: Record<number, string> = { 1: "нормальное", 2: "жёлтое", 4: "красное" };

/** "45 с", "10 мин", "1 ч", "1 ч 30 мин". */
function fmtSecs(s: number): string {
  if (s < 60) return `${s} с`;
  if (s < 3600) return s % 60 ? `${Math.floor(s / 60)} мин ${s % 60} с` : `${s / 60} мин`;
  const m = Math.round(s / 60);
  return m % 60 ? `${Math.floor(m / 60)} ч ${m % 60} мин` : `${m / 60} ч`;
}

const LEVELS_RED = [
  { value: 2, label: "жёлтое" },
  { value: 4, label: "красное" },
];
const LEVELS_ALL = [{ value: 1, label: "любое" }, ...LEVELS_RED];

interface Props {
  /** Действующие настройки (живой статус сторожа). */
  cfg: GuardConfig;
  configFile: string;
  saving: boolean;
  onOpenConfig: () => void;
  onToast: (ok: boolean, text: string) => void;
}

/**
 * Настройки сторожа во вкладке «Сторож», по важности: когда снимать → кого → предохранители →
 * уведомления → правила. Каждая крутилка сразу пишет config.toml (`save_settings`): переключатели
 * и списки — по изменению, числа — по Enter или уходу с поля. Отказ бэкенда — тост и прежнее значение.
 */
export function GuardSettingsPanel({ cfg, configFile, saving, onOpenConfig, onToast }: Props) {
  async function apply(patch: Partial<GuardSettings>, done?: string): Promise<boolean> {
    try {
      await guardStore.patch(patch);
      if (done) onToast(true, done);
      return true;
    } catch (e) {
      onToast(false, errText(e));
      return false;
    }
  }
  const setRed = (patch: Partial<GuardConfig["red"]>) => apply({ red: { ...cfg.red, ...patch } });
  const setLimits = (limits: GuardLimit[]) => apply({ limits });

  const red = cfg.red;
  const swapGb = Math.round((red.swap_red_mb / 1024) * 10) / 10;
  const kinds = [...new Set(cfg.allow.map((a) => a.name))].join(", ");
  const limits = cfg.limits;

  return (
    <div className="gset">
      <section className="gset-block">
        <h3 className="sec-title">Когда снимать — красная зона, любое из условий</h3>
        <div className="fields">
          <SelectField
            label="Давление памяти от"
            value={red.pressure_level}
            options={LEVELS_RED}
            onChange={(v) => void setRed({ pressure_level: v })}
          />
          <NumField label="Своп от" unit="ГБ" value={swapGb} onCommit={(v) => setRed({ swap_red_mb: Math.round(v * 1024) })} />
          <SelectField
            label="…если давление"
            value={red.swap_red_from_level}
            options={LEVELS_ALL}
            onChange={(v) => void setRed({ swap_red_from_level: v })}
          />
          <NumField
            label="Доступно не больше"
            unit="%"
            value={red.min_available_pct}
            onCommit={(v) => setRed({ min_available_pct: v })}
            hint="страховка на случай, когда давление молчит"
          />
        </div>
        <div className="fields">
          <NumField
            label="Красное держится"
            unit="с"
            value={cfg.sustain_secs}
            onCommit={(v) => apply({ sustain_secs: v })}
            hint="короткие всплески не в счёт"
          />
        </div>
        <p className="dim small gset-sum">
          Сейчас: давление {LEVEL_LABEL[red.pressure_level] ?? red.pressure_level} и выше, или своп от {fmtBytes(red.swap_red_mb * MB)}
          {red.swap_red_from_level > 1 ? ` (если давление ${LEVEL_LABEL[red.swap_red_from_level]} и выше)` : " (при любом давлении)"}, или доступно ≤{" "}
          {red.min_available_pct} % — и так {fmtSecs(cfg.sustain_secs)} подряд.
        </p>
      </section>

      <section className="gset-block">
        <h3 className="sec-title">Кого снимать</h3>
        <div className="fields">
          <NumField
            label="Жертва от"
            unit="МБ"
            value={cfg.min_victim_mb}
            onCommit={(v) => apply({ min_victim_mb: v })}
            hint="меньшие прогоны машину не спасут"
          />
          <NumField
            label="После SIGTERM ждать"
            unit="с"
            value={cfg.term_grace_secs}
            onCommit={(v) => apply({ term_grace_secs: v })}
            hint="потом SIGKILL"
          />
        </div>
        <p className="dim small gset-sum">
          Самый большой прогон из «можно» ({kinds || "правил нет"}). Приложения, сессии и «никогда» ({cfg.never.length}) не
          трогает.
        </p>
      </section>

      <section className="gset-block">
        <h3 className="sec-title">Предохранители</h3>
        <div className="limits">
          {limits.map((l, i) => (
            <div key={`${i}-${limits.length}`} className="limit-row">
              <span>не больше</span>
              <NumField
                label=""
                value={l.max}
                onCommit={(v) => setLimits(limits.map((x, j) => (j === i ? { ...x, max: v } : x)))}
              />
              <span>снятий за</span>
              <NumField
                label=""
                unit="с"
                value={l.window_secs}
                onCommit={(v) => setLimits(limits.map((x, j) => (j === i ? { ...x, window_secs: v } : x)))}
              />
              <span className="dim small">{l.window_secs > 0 ? `= ${fmtSecs(l.window_secs)}` : ""}</span>
              <button
                type="button"
                className="icon-btn"
                title="Убрать предел"
                aria-label="Убрать предел"
                disabled={saving}
                onClick={() => void setLimits(limits.filter((_, j) => j !== i))}
              >
                ×
              </button>
            </div>
          ))}
          <button
            type="button"
            className="btn-link"
            disabled={saving}
            onClick={() => void setLimits([...limits, { window_secs: 3600, max: 5 }])}
          >
            + Добавить предел
          </button>
        </div>
        <div className="fields">
          <NumField
            label="Пауза после снятия"
            unit="с"
            value={cfg.after_kill_secs}
            onCommit={(v) => apply({ after_kill_secs: v })}
            hint="дать памяти освободиться"
          />
        </div>
      </section>

      <section className="gset-block">
        <h3 className="sec-title">Уведомления</h3>
        <Switch label="О каждом снятии" checked={cfg.notify} disabled={saving} onChange={(v) => void apply({ notify: v })} />
        <Switch
          label="В режиме пробы — кого снял бы"
          checked={cfg.notify_dry_run}
          disabled={saving}
          onChange={(v) => void apply({ notify_dry_run: v })}
        />
      </section>

      <section className="gset-block">
        <details className="rules">
          <summary>
            Правила: «можно» ({cfg.allow.length}) и «никогда» ({cfg.never.length}) — правятся в config.toml
          </summary>
          <div className="rules-cols">
            <div>
              <h4>Можно снимать</h4>
              <ul>
                {cfg.allow.map((r, i) => (
                  <li key={i}>
                    <b>{r.name}</b> <code>exe {r.exe}</code>
                    {r.args && <code>args {r.args}</code>}
                    {r.args_not && <code>кроме {r.args_not}</code>}
                    {r.parent_not && <code>родитель не {r.parent_not}</code>}
                  </li>
                ))}
              </ul>
            </div>
            <div>
              <h4>Никогда</h4>
              <ul>
                {cfg.never.map((r, i) => (
                  <li key={i}>
                    <b>{r.name}</b>
                    {r.exe && <code>exe {r.exe}</code>}
                    {r.path && <code>путь {r.path}</code>}
                    {r.args && <code>args {r.args}</code>}
                  </li>
                ))}
              </ul>
            </div>
          </div>
        </details>
        <div className="config-line">
          <button className="btn btn-small" onClick={onOpenConfig}>
            Открыть config.toml
          </button>
          <span className="dim small mono ellipsis" title={configFile}>
            {configFile}
          </span>
        </div>
      </section>
    </div>
  );
}
