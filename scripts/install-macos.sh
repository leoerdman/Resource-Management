#!/usr/bin/env bash
# Собрать и установить Resource Monitor на этот Mac:
#   /Applications/Resource Monitor.app  (окно + значок в строке меню + CLI внутри)
#   ~/.local/bin/rmon → CLI из бандла (для терминала и `claude mcp add`)
# Повторный запуск обновляет установку. Подпись — ad-hoc (без Developer ID).
#   scripts/install-macos.sh         — собрать и поставить .app
#   scripts/install-macos.sh --dmg   — ещё и установщик .dmg для раздачи (target/release/bundle/dmg)
set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="Resource Monitor"
BUNDLE="target/release/bundle/macos/$APP_NAME.app"
DEST="/Applications/$APP_NAME.app"
BIN_DIR="${RMON_BIN_DIR:-$HOME/.local/bin}"

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
BUNDLES=app
[ "${1:-}" = "--dmg" ] && BUNDLES=app,dmg
( cd apps/rmon-app && npm ci --no-audit --no-fund && npx tauri build --bundles "$BUNDLES" )

codesign --force --deep --sign - "$BUNDLE"

if pgrep -f "$DEST/Contents/MacOS/rmon-app" >/dev/null; then
  osascript -e "tell application \"$APP_NAME\" to quit" >/dev/null 2>&1 || true
  for _ in $(seq 1 20); do pgrep -f "$DEST/Contents/MacOS/rmon-app" >/dev/null || break; sleep 0.25; done
  # Не вышло вежливо — снимаем свой же старый экземпляр (сторож в нём отпустит блокировку).
  pkill -f "$DEST/Contents/MacOS/rmon-app" 2>/dev/null || true
  sleep 1
fi
rm -rf "$DEST"
cp -R "$BUNDLE" "$DEST"
xattr -cr "$DEST"

mkdir -p "$BIN_DIR"
ln -sf "$DEST/Contents/MacOS/rmon" "$BIN_DIR/rmon"

open "$DEST"
echo "Установлено: $DEST"
echo "CLI: $BIN_DIR/rmon ($("$BIN_DIR/rmon" --version))"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) echo "Добавьте $BIN_DIR в PATH, чтобы вызывать rmon из терминала." ;; esac
echo "Подключить к Claude Code: claude mcp add --scope user rmon -- \"$DEST/Contents/MacOS/rmon\" mcp"
