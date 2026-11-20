//! Группировка процессов по приложению.
//!
//! Процесс внутри `/Applications/X.app/...` (или любого другого `X.app`) относится к «X».
//! Хелперы получают имя родительского приложения: «Google Chrome Helper (Renderer)» →
//! «Google Chrome», «M Code Desktop Helper» → «M Code Desktop». Прочее живёт под своим именем.
//!
//! Linux: `/snap/<имя>/…` → «имя», flatpak — по id приложения из cgroup ([`flatpak_app_id`]).
//! Windows: Electron/Squirrel-приложения в `%LOCALAPPDATA%\<X>\app-…` и
//! `%LOCALAPPDATA%\Programs\<X>\…` → «X»; иначе имя процесса без `.exe`.

use crate::model::ProcessInfo;

/// Хелперы приложений, у которых `.app` не видно в пути (запущены из фреймворка или обёртки).
const NAME_ALIASES: &[(&str, &str)] = &[
    ("Google Chrome Helper", "Google Chrome"),
    ("M Code Desktop Helper", "M Code Desktop"),
    ("M Code Helper", "M Code Desktop"),
    ("Claude Helper", "Claude"),
    ("Code Helper", "Visual Studio Code"),
    ("Slack Helper", "Slack"),
    ("Telegram Helper", "Telegram"),
];

/// Имя приложения для процесса: `X.app` из пути, иначе имя процесса.
pub fn app_group(name: &str, exe: Option<&str>) -> String {
    if let Some(stem) = exe.and_then(bundle_stem) {
        return canonical_bundle_name(stem).to_string();
    }
    if let Some(app) = exe.and_then(|e| snap_name(e).or_else(|| windows_app_dir(e))) {
        return app.to_string();
    }
    let name = name.trim();
    let name = strip_exe(name);
    for (prefix, group) in NAME_ALIASES {
        if name.starts_with(prefix) {
            return (*group).to_string();
        }
    }
    if name.is_empty() {
        "неизвестный".to_string()
    } else {
        name.to_string()
    }
}

/// `.exe` на конце (без учёта регистра) — Windows-имена группируются без него.
fn strip_exe(name: &str) -> &str {
    match name.len().checked_sub(4) {
        Some(i) if i > 0 && name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".exe") => &name[..i],
        _ => name,
    }
}

/// Snap: `/snap/<имя>/<ревизия>/…` → имя.
fn snap_name(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/snap/")?;
    let (name, tail) = rest.split_once('/')?;
    (!name.is_empty() && !tail.is_empty() && name != "bin").then_some(name)
}

/// Windows: каталог приложения в `AppData\Local` — `Programs\<X>\…` или Squirrel `<X>\app-1.2.3\…`.
fn windows_app_dir(path: &str) -> Option<&str> {
    let lower = path.to_ascii_lowercase();
    let idx = lower.find("\\appdata\\local\\")?;
    let rest = &path[idx + "\\appdata\\local\\".len()..];
    let mut parts = rest.split('\\');
    let first = parts.next()?;
    let second = parts.next()?;
    // Последний компонент — сам файл: каталог приложения должен быть выше него.
    let has_more = parts.next().is_some();
    if first.eq_ignore_ascii_case("programs") && has_more && !second.is_empty() {
        return Some(second);
    }
    let squirrel = second.len() > 4 && second[..4].eq_ignore_ascii_case("app-") && second[4..].starts_with(|c: char| c.is_ascii_digit());
    (squirrel && has_more && !first.is_empty()).then_some(first)
}

/// Flatpak: id приложения из `/proc/<pid>/cgroup` (`…/app-flatpak-org.telegram.desktop-12345.scope`).
pub fn flatpak_app_id(cgroup: &str) -> Option<&str> {
    let start = cgroup.find("app-flatpak-")? + "app-flatpak-".len();
    let rest = &cgroup[start..];
    let end = rest.find(".scope").unwrap_or(rest.len());
    let unit = &rest[..end];
    // Хвост `-<число>` — номер экземпляра.
    let id = match unit.rsplit_once('-') {
        Some((id, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => id,
        _ => unit,
    };
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// То же по готовому процессу.
pub fn app_group_for(process: &ProcessInfo) -> String {
    if process.app_group.is_empty() {
        app_group(&process.name, process.exe.as_deref())
    } else {
        process.app_group.clone()
    }
}

/// Имя приложения из пути: первый (самый внешний) компонент `*.app` в пути.
pub fn bundle_stem(path: &str) -> Option<&str> {
    for component in path.split('/') {
        if let Some(stem) = component
            .strip_suffix(".app")
            .or_else(|| component.strip_suffix(".APP"))
            && !stem.is_empty()
        {
            return Some(stem);
        }
    }
    None
}

/// Путь лежит внутри бандла приложения.
pub fn in_app_bundle(path: &str) -> bool {
    bundle_stem(path).is_some()
}

fn canonical_bundle_name(stem: &str) -> &str {
    let stem = trim_version_suffix(stem);
    match stem.find(" Helper") {
        Some(idx) => stem[..idx].trim_end(),
        None => stem,
    }
}

fn trim_version_suffix(stem: &str) -> &str {
    let Some(idx) = stem.rfind(' ') else {
        return stem;
    };
    let tail = &stem[idx + 1..];
    if !tail.is_empty()
        && tail.chars().all(|c| c.is_ascii_digit() || c == '.')
        && tail.chars().any(|c| c.is_ascii_digit())
    {
        stem[..idx].trim_end()
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_app_from_bundle_path() {
        assert_eq!(
            app_group(
                "Google Chrome",
                Some("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
            ),
            "Google Chrome"
        );
        assert_eq!(
            app_group(
                "Telegram",
                Some("/Applications/Telegram.app/Contents/MacOS/Telegram")
            ),
            "Telegram"
        );
    }

    #[test]
    fn helpers_belong_to_parent_app() {
        assert_eq!(
            app_group(
                "Google Chrome Helper (Renderer)",
                Some("/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/140.0/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)")
            ),
            "Google Chrome"
        );
        assert_eq!(
            app_group(
                "M Code Desktop Helper (Renderer)",
                Some("/Applications/M Code Desktop.app/Contents/Frameworks/M Code Desktop Helper (Renderer).app/Contents/MacOS/M Code Desktop Helper (Renderer)")
            ),
            "M Code Desktop"
        );
        assert_eq!(
            app_group("M Code Desktop Helper (GPU)", None),
            "M Code Desktop"
        );
    }

    #[test]
    fn strips_version_from_bundle_name() {
        assert_eq!(
            app_group("X", Some("/Applications/M Code Desktop 1.4.2.app/Contents/MacOS/x")),
            "M Code Desktop"
        );
    }

    #[test]
    fn falls_back_to_process_name() {
        assert_eq!(
            app_group(
                "node",
                Some("/opt/homebrew/Cellar/node/25/bin/node")
            ),
            "node"
        );
        assert_eq!(app_group("claude", None), "claude");
        assert_eq!(app_group("", None), "неизвестный");
    }

    #[test]
    fn sees_bundles_anywhere() {
        assert_eq!(
            bundle_stem("/Users/x/Applications/Foo.app/Contents/MacOS/Foo"),
            Some("Foo")
        );
        assert_eq!(bundle_stem("/usr/bin/yes"), None);
        assert!(in_app_bundle("/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder"));
    }

    #[test]
    fn linux_snap_and_flatpak() {
        assert_eq!(app_group("firefox", Some("/snap/firefox/4793/usr/lib/firefox/firefox")), "firefox");
        assert_eq!(app_group("node", Some("/snap/bin/node")), "node");
        assert_eq!(
            flatpak_app_id("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-org.telegram.desktop-51234.scope\n"),
            Some("org.telegram.desktop")
        );
        assert_eq!(flatpak_app_id("0::/user.slice/user-1000.slice/session-2.scope"), None);
    }

    #[test]
    fn windows_names_and_dirs() {
        assert_eq!(app_group("chrome.exe", Some(r"C:\Program Files\Google\Chrome\Application\chrome.exe")), "chrome");
        assert_eq!(app_group("Discord.exe", Some(r"C:\Users\x\AppData\Local\Discord\app-1.0.9170\Discord.exe")), "Discord");
        assert_eq!(app_group("Cursor.exe", Some(r"C:\Users\x\AppData\Local\Programs\cursor\Cursor.exe")), "cursor");
        assert_eq!(app_group("node.EXE", None), "node");
        assert_eq!(app_group("x.exe", Some(r"C:\Users\x\AppData\Local\Temp\x.exe")), "x");
        assert_eq!(app_group(".exe", None), ".exe");
    }
}
