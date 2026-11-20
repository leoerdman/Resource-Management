# Роадмап rmon

## Сейчас — macOS (приоритет владельца, 2026-11-20)

- [x] Ядро: сбор (libproc/mach/sysctl), история, анализ, политика, снятие, `Monitor`
- [x] CLI `rmon`: status / ps / suggest / why / kill / config
- [x] Окно Resource Monitor (Tauri) и значок в строке меню
- [x] Вид списком, группы брошенных копий, панель подробностей процесса
- [x] MCP-сервер для Claude Code (`rmon mcp`)
- [x] Сторож памяти перенесён из `Hooks/memory-guard` в rmon (`rmon_core::guard`)
- [x] Запуск при входе в macOS (LaunchAgent приложения), настройки из окна с записью в config.toml
- [x] Вкладки «Сторож» и «Настройки» в окне, «Снять все» одной командой (параллельно)
- [x] CI на macOS: формат, clippy, тесты, сверка TS-типов, фронтенд
- [ ] Перенос обязанности со старого LaunchAgent `com.leoerdman.memory-guard` (кнопка в окне)

## Позже — Linux и Windows

Отложено решением владельца 2026-11-20: сначала — Mac. Задел уже есть:

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
