#!/usr/bin/env bash
# Build Onix and install it for the current user.
set -euo pipefail

cd "$(dirname "$0")"

APP_ID="dev.onix.Onix"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"

cargo build --release

install -Dm755 target/release/onix "$BIN_DIR/onix"
install -Dm644 "data/$APP_ID.svg" "$ICON_DIR/$APP_ID.svg"

# The desktop file is named after the app ID so the desktop can match it, and its icon, to
# the app's windows. It's executable because KDE Plasma won't launch it otherwise. Exec is an
# absolute path so the launcher works even if ~/.local/bin isn't on PATH.
mkdir -p "$APP_DIR"
sed "s|^Exec=.*|Exec=$BIN_DIR/onix|" "data/$APP_ID.desktop" > "$APP_DIR/$APP_ID.desktop"
chmod 755 "$APP_DIR/$APP_ID.desktop"
# Left by earlier versions of this script.
rm -f "$APP_DIR/Onix.desktop"

# Refresh the launcher menus and icons, for whichever desktop is in use. Not
# update-desktop-database: it only indexes MIME types, and Onix doesn't open files.
if [ -f "$HOME/.local/share/icons/hicolor/index.theme" ]; then
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" || true
fi
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
touch "$HOME/.local/share/icons/hicolor"

echo "Installed onix to $BIN_DIR/onix"
echo "Installed launcher to $APP_DIR/$APP_ID.desktop"
