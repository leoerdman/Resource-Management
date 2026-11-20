//! Форматирование для человека: размеры и проценты по-русски, с запятой.
//!
//! Один источник правды для `reason` подозреваемых и для таблиц CLI: «3,1 ГБ» пишется
//! одним и тем же кодом везде.

use crate::model::{GIB, KIB, MIB};

const UNITS: [(&str, u64); 4] = [
    ("ТБ", GIB * 1024),
    ("ГБ", GIB),
    ("МБ", MIB),
    ("КБ", KIB),
];

/// Размер человеку: «3,1 ГБ», «412 МБ», «980 КБ», «512 Б».
pub fn bytes(n: u64) -> String {
    if n < KIB {
        return format!("{n} Б");
    }
    for (suffix, scale) in UNITS {
        if n >= scale {
            return format!("{} {suffix}", scaled(n, scale));
        }
    }
    format!("{n} Б")
}

/// Скорость роста: «40,0 МБ/мин».
pub fn bytes_per_min(per_min: f64) -> String {
    let unit = if per_min.abs() >= MIB as f64 {
        (MIB as f64, "МБ/мин")
    } else {
        (KIB as f64, "КБ/мин")
    };
    format!("{} {}", decimal1(per_min / unit.0), unit.1)
}

/// Длительность коротко: «6 мин», «2 ч 10 мин», «12 с».
pub fn duration_human(seconds: u64) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        if minutes == 0 {
            format!("{hours} ч")
        } else {
            format!("{hours} ч {minutes} мин")
        }
    } else if seconds >= 60 {
        format!("{} мин", seconds / 60)
    } else {
        format!("{seconds} с")
    }
}

/// Проценты с одним знаком: «13,4 %».
pub fn percent(value: f64) -> String {
    format!("{} %", decimal1(value))
}

fn scaled(n: u64, scale: u64) -> String {
    let tenths = (u128::from(n) * 10 + u128::from(scale / 2)) / u128::from(scale);
    let whole = tenths / 10;
    let frac = tenths % 10;
    if frac == 0 {
        whole.to_string()
    } else {
        format!("{whole},{frac}")
    }
}

fn decimal1(value: f64) -> String {
    let tenths = (value * 10.0).round().max(0.0) as u64;
    let whole = tenths / 10;
    let frac = tenths % 10;
    if frac == 0 {
        whole.to_string()
    } else {
        format!("{whole},{frac}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_gigabytes_with_comma() {
        assert_eq!(bytes(3_328_600_064), "3,1 ГБ");
        assert_eq!(bytes(3 * GIB), "3 ГБ");
        assert_eq!(bytes(432 * MIB), "432 МБ");
        assert_eq!(bytes(512), "512 Б");
    }

    #[test]
    fn rounds_to_nearest_tenth() {
        assert_eq!(bytes(1_073_741_824 + 100), "1 ГБ");
        assert_eq!(bytes(3 * GIB / 10), "307,2 МБ");
        assert_eq!(bytes(24 * GIB), "24 ГБ");
    }

    #[test]
    fn prints_speed_and_duration() {
        assert_eq!(bytes_per_min(40.0 * MIB as f64), "40 МБ/мин");
        assert_eq!(bytes_per_min(512.0 * 1024.0), "512 КБ/мин");
        assert_eq!(duration_human(6 * 60), "6 мин");
        assert_eq!(duration_human(2 * 3600 + 10 * 60), "2 ч 10 мин");
        assert_eq!(duration_human(12), "12 с");
    }

    #[test]
    fn prints_percent() {
        assert_eq!(percent(13.42), "13,4 %");
        assert_eq!(percent(4.0), "4 %");
    }
}
