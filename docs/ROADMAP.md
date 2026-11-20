# Роадмап rmon

## Сейчас — macOS

- [x] Ядро: сбор (libproc/mach/sysctl), история, анализ, политика, снятие, `Monitor`
- [x] CLI `rmon`: status / ps / suggest / why / kill / config
- [x] Окно Resource Monitor (Tauri) и значок в строке меню
- [x] Вид списком, группы брошенных копий, панель подробностей процесса
- [x] MCP-сервер для Claude Code (`rmon mcp`)
- [x] Сторож памяти в приложении (`rmon_core::guard`)
- [x] Запуск при входе в macOS (LaunchAgent приложения), настройки из окна с записью в config.toml
- [x] Вкладки «Сторож» и «Настройки» в окне, «Снять все» одной командой (параллельно)
- [x] CI на macOS: формат, clippy, тесты, сверка TS-типов, фронтенд
- [x] Цифры памяти сверены с Activity Monitor синхронными замерами; «Занято» = всего − свободно − кэш
- [x] Иконка: шкала давления (`apps/rmon-app/assets/icon.svg`)
- [x] Журналы хранятся 24 ч (`[logs]`), лента состояния раз в минуту, уведомление при упоре сторожа в предел
- [x] Настройки сторожа — крутилками прямо во вкладке «Сторож», по важности; правка сразу в config.toml
- [x] Автоочистка сирот в простое (вторая карточка вкладки «Сторож»)

## Позже

- Группировка XPC-хелперов по ответственному приложению (`responsibility_get_pid_responsible_for_pid`), как в Activity Monitor: `com.apple.WebKit.WebContent` сейчас отдельной строкой.
- Поле «Хранить журналы, часов» во вкладке «Настройки» (сейчас — `[logs]` в config.toml).
- Списки сторожа «можно» и «никогда» из окна (сейчас — в config.toml, в окне только просмотр).

## Позже — Linux и Windows

Сначала — Mac. Задел уже есть:

- Всё выше трейта `Platform` платформонезависимо; политика, конфиг и пути разведены по ОС
  (`policy::builtin_protect`, `guard::default_never`, `config::log_dir`/`config_dir`).
- Начатая работа — ветка `worktree-agent-aa473de5ed3207a1d` (WIP): `platform/linux.rs`
  (`/proc`, PSI), `platform/windows.rs` (`windows-sys`), CI на трёх ОС, релиз через tauri-action.
  Перед продолжением — перебазировать на `main` и учесть: sidecar CLI (`scripts/prepare-sidecar.mjs`
  перед `cargo` в CI), drift-check `apps/rmon-app/src/bindings` (типы, `labels.json`,
  `format-cases.json`), `npm test`.
- Сторож на Linux/Windows: зона по PSI / memory load, автозапуск через systemd user unit и
  Run-ключ / Task Scheduler (`tauri-plugin-autostart` это умеет).
- Установщики `.msi`/`.AppImage`/`.deb` — `bundle.targets: "all"` уже стоит.
