//! Форматирование для человека: размеры и проценты по-русски, с запятой.
//!
//! Один источник правды для `reason` подозреваемых и для таблиц CLI: «3,1 ГБ» пишется
//! одним и тем же кодом везде.

use crate::model::{KIB, MIB};

const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];

/// Размер человеку: «3,1 ГБ», «412 МБ», «980 КБ», «512 Б».
///
/// Правило (оно же в окне — `apps/rmon-app/src/format.ts`, сверяется тестом по
/// `bindings/format-cases.json`, который пишет [`crate::contract`]): основание 1024;
/// единица меняется, когда округлённое значение дошло бы до 1000 («1 ГБ», а не «1003 МБ»
/// и не «1000 МБ»); от 100 — целое,
/// ниже — один знак после запятой, «,0» отбрасывается.
pub fn bytes(n: u64) -> String {
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 999.5 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    let body = if unit == 0 || v >= 100.0 { format!("{}", v.round() as u64) } else { decimal1(v) };
    format!("{body} {}", UNITS[unit])
}

/// Скорость роста: «40 МБ/мин», убыль — со знаком: «-1,5 МБ/мин».
pub fn bytes_per_min(per_min: f64) -> String {
    let unit = if per_min.abs() >= MIB as f64 { (MIB as f64, "МБ/мин") } else { (KIB as f64, "КБ/мин") };
    let value = decimal1(per_min.abs() / unit.0);
    let sign = if per_min < 0.0 && value != "0" { "-" } else { "" };
    format!("{sign}{value} {}", unit.1)
}

/// Изменение размера со знаком: «+1,2 ГБ», «-300 МБ», «0 Б».
pub fn bytes_delta(delta: i64) -> String {
    let sign = match delta.signum() {
        1 => "+",
        -1 => "-",
        _ => "",
    };
    format!("{sign}{}", bytes(delta.unsigned_abs()))
}

/// Длительность коротко: «6 мин», «2 ч 10 мин», «12 с».
pub fn duration_human(seconds: u64) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        if minutes == 0 { format!("{hours} ч") } else { format!("{hours} ч {minutes} мин") }
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

/// Один знак после запятой: десятые — `round(v * 10)`, ровно как `Math.round` в окне.
fn decimal1(value: f64) -> String {
    let tenths = (value * 10.0).round().max(0.0) as u64;
    let whole = tenths / 10;
    let frac = tenths % 10;
    if frac == 0 { whole.to_string() } else { format!("{whole},{frac}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GIB;

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
        assert_eq!(bytes(3 * GIB / 10), "307 МБ");
        assert_eq!(bytes(1003 * MIB), "1 ГБ");
        assert_eq!(bytes(99 * MIB + 512 * KIB), "99,5 МБ");
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
    fn keeps_sign_of_shrink_and_delta() {
        assert_eq!(bytes_per_min(-1.5 * MIB as f64), "-1,5 МБ/мин");
        assert_eq!(bytes_per_min(-0.01), "0 КБ/мин");
        assert_eq!(bytes_delta(1288 * MIB as i64), format!("+{}", bytes(1288 * MIB)));
        assert_eq!(bytes_delta(-300 * MIB as i64), format!("-{}", bytes(300 * MIB)));
        assert_eq!(bytes_delta(0), bytes(0));
        assert_eq!(bytes_delta(i64::MIN), format!("-{}", bytes(i64::MIN.unsigned_abs())));
    }

    #[test]
    fn prints_percent() {
        assert_eq!(percent(13.42), "13,4 %");
        assert_eq!(percent(4.0), "4 %");
    }
}
