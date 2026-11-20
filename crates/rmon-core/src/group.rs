//! Группировка процессов по приложению.
//!
//! Процесс внутри `/Applications/X.app/...` (или любого другого `X.app`) относится к «X».
//! Хелперы получают имя родительского приложения: «Google Chrome Helper (Renderer)» →
//! «Google Chrome», «M Code Desktop Helper» → «M Code Desktop». Прочее живёт под своим именем.

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
    let name = name.trim();
    for (prefix, group) in NAME_ALIASES {
        if name.starts_with(prefix) {
            return (*group).to_string();
        }
    }
    if name.is_empty() { "неизвестный".to_string() } else { name.to_string() }
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
        if let Some(stem) = component.strip_suffix(".app").or_else(|| component.strip_suffix(".APP"))
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
            app_group("Google Chrome", Some("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")),
            "Google Chrome"
        );
        assert_eq!(app_group("Telegram", Some("/Applications/Telegram.app/Contents/MacOS/Telegram")), "Telegram");
    }

    #[test]
    fn helpers_belong_to_parent_app() {
        assert_eq!(
            app_group(
                "Google Chrome Helper (Renderer)",
                Some(
                    "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/140.0/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)"
                )
            ),
            "Google Chrome"
        );
        assert_eq!(
            app_group(
                "M Code Desktop Helper (Renderer)",
                Some(
                    "/Applications/M Code Desktop.app/Contents/Frameworks/M Code Desktop Helper (Renderer).app/Contents/MacOS/M Code Desktop Helper (Renderer)"
                )
            ),
            "M Code Desktop"
        );
        assert_eq!(app_group("M Code Desktop Helper (GPU)", None), "M Code Desktop");
    }

    #[test]
    fn strips_version_from_bundle_name() {
        assert_eq!(app_group("X", Some("/Applications/M Code Desktop 1.4.2.app/Contents/MacOS/x")), "M Code Desktop");
    }

    #[test]
    fn falls_back_to_process_name() {
        assert_eq!(app_group("node", Some("/opt/homebrew/Cellar/node/25/bin/node")), "node");
        assert_eq!(app_group("claude", None), "claude");
        assert_eq!(app_group("", None), "неизвестный");
    }

    #[test]
    fn sees_bundles_anywhere() {
        assert_eq!(bundle_stem("/Users/x/Applications/Foo.app/Contents/MacOS/Foo"), Some("Foo"));
        assert_eq!(bundle_stem("/usr/bin/yes"), None);
        assert!(in_app_bundle("/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder"));
    }
}
