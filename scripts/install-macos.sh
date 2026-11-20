#!/usr/bin/env bash
# Собрать и установить Resource Monitor на этот Mac:
#   /Applications/Resource Monitor.app  (окно + значок в строке меню + CLI внутри)
#   ~/.local/bin/rmon → CLI из бандла (для терминала и `claude mcp add`)
# Повторный запуск обновляет установку. Подпись — ad-hoc (без Developer ID).
set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="Resource Monitor"
BUNDLE="target/release/bundle/macos/$APP_NAME.app"
DEST="/Applications/$APP_NAME.app"
BIN_DIR="${RMON_BIN_DIR:-$HOME/.local/bin}"

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
( cd apps/rmon-app && npm ci --no-audit --no-fund && npx tauri build --bundles app,dmg )

codesign --force --deep --sign - "$BUNDLE"

if pgrep -f "$DEST/Contents/MacOS/rmon-app" >/dev/null; then
  osascript -e "tell application \"$APP_NAME\" to quit" >/dev/null 2>&1 || true
  for _ in $(seq 1 20); do pgrep -f "$DEST/Contents/MacOS/rmon-app" >/dev/null || break; sleep 0.25; done
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
