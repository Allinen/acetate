#!/bin/sh
# Installs the desktop client for the current user: the program, its icon and its menu
# entry. On Wayland (KDE Plasma, GNOME) a window can't set its own icon: the desktop
# finds it through the `.desktop` file whose name matches the app id ("acetate").
#
# Everything goes into your home folder (~/.local), nothing system-wide. Run it again
# after pulling changes. To take it out again, remove the three files it lists.
set -eu
cd "$(dirname "$0")"

cargo build --release
bin="$HOME/.local/bin/acetate-desktop"
icon="$HOME/.local/share/icons/hicolor/512x512/apps/acetate.png"
entry="$HOME/.local/share/applications/acetate.desktop"
mkdir -p "$(dirname "$bin")" "$(dirname "$icon")" "$(dirname "$entry")"
install -m 755 target/release/acetate-desktop "$bin"
install -m 644 assets/acetate-512.png "$icon"
# The entry runs the installed program by its full path (~/.local/bin may not be on PATH).
sed "s|^Exec=.*|Exec=$bin|" acetate.desktop > "$entry"

# Tell the desktop about it (whichever of these exist).
command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications" || true
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" || true

echo "Installed:"
echo "  $bin"
echo "  $icon"
echo "  $entry"
echo "Start Acetate from your app menu (or run $bin)."
