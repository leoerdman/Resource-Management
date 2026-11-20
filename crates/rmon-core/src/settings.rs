//! Настройки, которые человек меняет из окна, — срез [`Config`] с записью обратно в `config.toml`.
//!
//! Файл остаётся единственным источником истины: окно читает [`Settings::current`], пишет
//! [`save`], а всё, что в окне не редактируется (списки правил сторожа и политики, пути), в файле
//! сохраняется как было — вместе с комментариями человека (`toml_edit`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::analyze::Thresholds;
use crate::config::{self, Config};
use crate::guard::{Limit, RedConfig};

/// Настройки приложения (секция `[app]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
#[ts(export)]
pub struct AppConfig {
    /// Запускать Resource Monitor при входе в систему (на macOS — LaunchAgent приложения).
    pub autostart: bool,
    /// Показывать «занято X ГБ» рядом со значком в строке меню.
    pub tray_title: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self { autostart: true, tray_title: true }
    }
}

/// Редактируемая из окна часть настроек сторожа.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GuardSettings {
    /// Сторож работает.
    pub enabled: bool,
    /// Проба.
    pub dry_run: bool,
    /// Уведомлять о снятии.
    pub notify: bool,
    /// Уведомлять в пробе.
    pub notify_dry_run: bool,
    /// Красное должно держаться, с.
    #[ts(type = "number")]
    pub sustain_secs: u64,
    /// Пауза после снятия, с.
    #[ts(type = "number")]
    pub after_kill_secs: u64,
    /// От SIGTERM до SIGKILL, с.
    #[ts(type = "number")]
    pub term_grace_secs: u64,
    /// Меньше — не жертва, МБ.
    #[ts(type = "number")]
    pub min_victim_mb: u64,
    /// Красная зона.
    pub red: RedConfig,
    /// Пределы убийств.
    pub limits: Vec<Limit>,
}

/// Пороги анализа — в окне в человеческих единицах.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, rename = "ThresholdSettings")]
pub struct ThresholdSettings {
    /// Большой: от, МБ.
    #[ts(type = "number")]
    pub hog_mb: u64,
    /// Большой: от, % ОЗУ.
    pub hog_pct: f64,
    /// Растёт: от, МБ/мин.
    pub growing_mb_per_min: f64,
    /// Простаивает: от, МБ.
    #[ts(type = "number")]
    pub idle_mb: u64,
    /// Простаивает: живёт дольше, мин.
    #[ts(type = "number")]
    pub idle_age_min: u64,
    /// Дубли: от стольких брошенных копий.
    pub duplicate_count: u32,
}

/// Всё, что меняется из окна.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Settings {
    /// Приложение.
    pub app: AppConfig,
    /// Сторож.
    pub guard: GuardSettings,
    /// Пороги подозреваемых.
    pub thresholds: ThresholdSettings,
    /// Где лежит файл (только чтение).
    pub config_file: String,
}

const MIB: u64 = 1024 * 1024;

impl Settings {
    /// Срез конфигурации.
    pub fn from_config(c: &Config) -> Self {
        let g = &c.guard;
        let t = &c.thresholds;
        Self {
            app: c.app.clone(),
            guard: GuardSettings {
                enabled: g.enabled,
                dry_run: g.dry_run,
                notify: g.notify,
                notify_dry_run: g.notify_dry_run,
                sustain_secs: g.sustain_secs,
                after_kill_secs: g.after_kill_secs,
                term_grace_secs: g.term_grace_secs,
                min_victim_mb: g.min_victim_mb,
                red: g.red.clone(),
                limits: g.limits.clone(),
            },
            thresholds: ThresholdSettings {
                hog_mb: t.hog_bytes / MIB,
                hog_pct: t.hog_pct,
                growing_mb_per_min: t.growing_per_min / MIB as f64,
                idle_mb: t.idle_bytes / MIB,
                idle_age_min: t.idle_age / 60,
                duplicate_count: t.duplicate_count as u32,
            },
            config_file: config::config_file().display().to_string(),
        }
    }

    /// Настройки из файла прямо сейчас.
    pub fn current() -> crate::Result<Self> {
        Self::current_from(&config::config_file())
    }

    /// Настройки из конкретного файла.
    pub fn current_from(path: &std::path::Path) -> crate::Result<Self> {
        let mut s = Self::from_config(&Config::load_from(path)?);
        s.config_file = path.display().to_string();
        Ok(s)
    }

    /// Применить к конфигурации (то, чего в окне нет, не трогается).
    pub fn apply_to(&self, c: &mut Config) {
        c.app = self.app.clone();
        let g = &mut c.guard;
        let s = &self.guard;
        g.enabled = s.enabled;
        g.dry_run = s.dry_run;
        g.notify = s.notify;
        g.notify_dry_run = s.notify_dry_run;
        g.sustain_secs = s.sustain_secs;
        g.after_kill_secs = s.after_kill_secs;
        g.term_grace_secs = s.term_grace_secs;
        g.min_victim_mb = s.min_victim_mb;
        g.red = s.red.clone();
        g.limits = s.limits.clone();
        let t = &mut c.thresholds;
        let s = &self.thresholds;
        t.hog_bytes = s.hog_mb * MIB;
        t.hog_pct = s.hog_pct;
        t.growing_per_min = s.growing_mb_per_min * MIB as f64;
        t.idle_bytes = s.idle_mb * MIB;
        t.idle_age = s.idle_age_min * 60;
        t.duplicate_count = s.duplicate_count as usize;
    }

    /// Проверка значений: человеку — понятная причина отказа.
    pub fn validate(&self) -> Result<(), String> {
        let g = &self.guard;
        let checks: [(bool, &str); 8] = [
            (g.sustain_secs <= 600, "красное должно держаться не дольше 10 минут"),
            (g.after_kill_secs <= 3600, "пауза после снятия — не больше часа"),
            (g.term_grace_secs <= 120, "ожидание после SIGTERM — не больше 2 минут"),
            (g.min_victim_mb >= 100, "жертва меньше 100 МБ машину не спасёт — порог от 100 МБ"),
            (matches!(g.red.pressure_level, 2 | 4), "уровень давления — 2 (жёлтое) или 4 (красное)"),
            (g.red.min_available_pct <= 50, "страховка «доступно не больше» — до 50 %"),
            (!g.limits.is_empty() && g.limits.iter().all(|l| l.max >= 1 && l.window_secs >= 10), "нужен хотя бы один предел убийств (окно от 10 с, от 1 убийства)"),
            (self.thresholds.duplicate_count >= 2, "дубли — от двух копий"),
        ];
        match checks.iter().find(|(ok, _)| !ok) {
            Some((_, why)) => Err((*why).to_string()),
            None => Ok(()),
        }
    }
}

/// Слить `src` в `dst`: таблицы — рекурсивно, значения — заменой. Ключи `dst`, которых нет
/// в `src`, и все комментарии остаются как были.
fn merge(dst: &mut toml_edit::Table, src: &toml_edit::Table) {
    for (key, item) in src.iter() {
        match (dst.get_mut(key), item) {
            (Some(toml_edit::Item::Table(d)), toml_edit::Item::Table(s)) => merge(d, s),
            // Значение меняем на месте: ключ с его комментарием остаётся, у значения — его отделка.
            (Some(toml_edit::Item::Value(d)), toml_edit::Item::Value(s)) => {
                let decor = d.decor().clone();
                *d = s.clone();
                *d.decor_mut() = decor;
            }
            (Some(d), _) => *d = item.clone(),
            (None, _) => {
                dst.insert(key, item.clone());
            }
        }
    }
}

/// Записать изменённую конфигурацию в `config.toml`, сохранив комментарии и всё прочее.
/// Пишется только то, что меняется из окна: `[app]`, флаги и числа `[guard]`, `[thresholds]`.
pub fn save(settings: &Settings) -> Result<Config, String> {
    save_to(&config::config_file(), settings)
}

/// То же в конкретный файл.
pub fn save_to(path: &std::path::Path, settings: &Settings) -> Result<Config, String> {
    settings.validate()?;
    let path = path.to_path_buf();
    let mut cfg = Config::load_from(&path).map_err(|e| e.to_string())?;
    settings.apply_to(&mut cfg);

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;

    #[derive(Serialize)]
    struct Patch<'a> {
        app: &'a AppConfig,
        thresholds: &'a Thresholds,
        guard: GuardPatch<'a>,
    }
    #[derive(Serialize)]
    struct GuardPatch<'a> {
        enabled: bool,
        dry_run: bool,
        notify: bool,
        notify_dry_run: bool,
        sustain_secs: u64,
        after_kill_secs: u64,
        term_grace_secs: u64,
        min_victim_mb: u64,
        red: &'a RedConfig,
        limits: &'a [Limit],
    }
    let g = &cfg.guard;
    let patch = Patch {
        app: &cfg.app,
        thresholds: &cfg.thresholds,
        guard: GuardPatch {
            enabled: g.enabled,
            dry_run: g.dry_run,
            notify: g.notify,
            notify_dry_run: g.notify_dry_run,
            sustain_secs: g.sustain_secs,
            after_kill_secs: g.after_kill_secs,
            term_grace_secs: g.term_grace_secs,
            min_victim_mb: g.min_victim_mb,
            red: &g.red,
            limits: &g.limits,
        },
    };
    let patch_text = toml::to_string(&patch).map_err(|e| e.to_string())?;
    let patch_doc: toml_edit::DocumentMut = patch_text.parse().map_err(|e| format!("{e}"))?;
    merge(doc.as_table_mut(), patch_doc.as_table());
    write_validated(&path, &doc.to_string())?;
    Ok(cfg)
}

/// Записать текст настроек атомарно, если rmon его разбирает.
pub fn write_validated(path: &std::path::Path, text: &str) -> Result<(), String> {
    Config::parse(text).map_err(|e| format!("после правки настройки не разбираются: {e}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_keeps_comments_and_rules() {
        let dir = std::env::temp_dir().join(format!("rmon-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        std::fs::write(
            &file,
            "# мой комментарий\n[[policy.confirm]]\nname = \"Obsidian\"\npath = \"/Obsidian\\\\.app/\"\n\n[guard]\n# порог\nmin_victim_mb = 2048\n",
        )
        .unwrap();
        let mut s = Settings::current_from(&file).unwrap();
        assert_eq!(s.guard.min_victim_mb, 2048);
        s.guard.dry_run = true;
        s.app.autostart = false;
        s.thresholds.hog_mb = 1500;
        save_to(&file, &s).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("# мой комментарий"));
        assert!(text.contains("# порог"));
        assert!(text.contains("Obsidian"));
        let back = Settings::current_from(&file).unwrap();
        assert!(back.guard.dry_run);
        assert!(!back.app.autostart);
        assert_eq!(back.thresholds.hog_mb, 1500);
        assert_eq!(back.guard.min_victim_mb, 2048);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn validation_explains() {
        let mut s = Settings::from_config(&Config::default());
        assert!(s.validate().is_ok());
        s.guard.min_victim_mb = 10;
        assert!(s.validate().unwrap_err().contains("100 МБ"));
    }
}
