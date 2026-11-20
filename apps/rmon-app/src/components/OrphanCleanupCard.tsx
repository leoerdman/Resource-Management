import { fmtBytes, fmtSecs, plural } from "../format";
import type { GuardOrphanWatch, GuardOrphans, GuardSettings } from "../types";
import { NumField, Switch } from "./Controls";

interface Props {
  cfg: GuardOrphans;
  watch: GuardOrphanWatch;
  /** Сторож снимает (боевой или проба) — без него автоочистка стоит. */
  guardActing: boolean;
  dryRun: boolean;
  saving: boolean;
  apply: (patch: Partial<GuardSettings>, done?: string) => Promise<boolean>;
}

/**
 * Вторая карточка вкладки «Сторож»: брошенные dev-процессы (родитель умер), которые простаивают
 * `idle_mins` минут, сторож снимает сам — в любой зоне. Переключатель и минуты сразу пишут
 * `[guard.orphans]` в config.toml.
 */
export function OrphanCleanupCard({ cfg, watch, guardActing, dryRun, saving, apply }: Props) {
  const set = (patch: Partial<GuardOrphans>, done?: string) => apply({ orphans: { ...cfg, ...patch } }, done);
  const on = cfg.enabled && guardActing;

  let state: string;
  if (!cfg.enabled) state = "выключена";
  else if (!guardActing) state = "стоит";
  else state = dryRun ? "проба" : "включена";

  let hint: string;
  if (!cfg.enabled) hint = "Сирот снимаете вы — кнопкой в «Подозреваемых» — или сторож в красной зоне.";
  else if (!guardActing) hint = "Сторож выключен или ждёт — автоочистка вместе с ним.";
  else
    hint = `Брошенный dev-процесс (родитель умер), который ${cfg.idle_mins} мин не тратит CPU и не меняет память, ${
      dryRun ? "попадёт в журнал как «снял бы»" : "снимается сам"
    } — в любой зоне.`;

  return (
    <section className="guard-head orphan-card">
      <div className="guard-mode">
        <div className="dim small">Автоочистка сирот</div>
        <div className={`mode-word ${on ? (dryRun ? "mode-dry_run" : "mode-armed") : "mode-off"}`}>{state}</div>
        <div className="dim">{hint}</div>
        {on && (
          <div className="zone-line">
            {watch.watched > 0 ? (
              <>
                Сирот сейчас <b>{watch.watched}</b>, вместе <b>{fmtBytes(watch.mem)}</b>
                {watch.next_in_secs != null && (
                  <span className="dim">
                    {" "}
                    — {watch.next_in_secs === 0 ? "ближайшая снимается" : `ближайшая через ${fmtSecs(watch.next_in_secs)}`}, если
                    так и простоит
                  </span>
                )}
              </>
            ) : (
              <span className="dim">Брошенных процессов сейчас нет.</span>
            )}
          </div>
        )}
        <div className="dim small">
          Не трогает приложения, сессии Claude Code и M Code, «никогда» и задачи LaunchAgent.
        </div>
      </div>
      <div className="guard-switches">
        <Switch
          big
          label="Снимать сирот в простое"
          checked={cfg.enabled}
          disabled={saving}
          onChange={(v) => void set({ enabled: v }, v ? "Автоочистка сирот включена" : "Автоочистка сирот выключена")}
        />
        <NumField
          label="Простой"
          unit={plural(cfg.idle_mins, "минута", "минуты", "минут")}
          value={cfg.idle_mins}
          min={1}
          onCommit={(v) => set({ idle_mins: Math.round(v) }, `Сирот — после ${Math.round(v)} мин простоя`)}
          hint="без CPU и движения памяти"
        />
      </div>
    </section>
  );
}
