#!/bin/sh
# Installs the desktop client for the current user: the program, its icon and its menu
# entry. On Wayland (KDE Plasma, GNOME) a window can't set its own icon: the desktop
# finds it through the `.desktop` file whose name matches the app id ("acetate").
#
# It also sets the app up for links to tracks, albums and artists (acetate://open/…, what
# a shared link's page hands over): the entry says it takes them, and it's made the one
# that opens them.
#
# Everything goes into your home folder (~/.local), nothing system-wide. Run it again
# after pulling changes. To take it out again, remove the three files it lists.
#
# From a release (package-linux.sh: the program is next to this script) it installs that;
# from the sources it builds it first.
set -eu
cd "$(dirname "$0")"

if [ -f acetate-desktop ]; then
    built=acetate-desktop
else
    cargo build --release
    built=../target/release/acetate-desktop
fi
bin="$HOME/.local/bin/acetate-desktop"
icon="$HOME/.local/share/icons/hicolor/512x512/apps/acetate.jpg"
entry="$HOME/.local/share/applications/acetate.desktop"
mkdir -p "$(dirname "$bin")" "$(dirname "$icon")" "$(dirname "$entry")"
install -m 755 "$built" "$bin"
install -m 644 assets/acetate-512.jpg "$icon"
# The entry runs the installed program by its full path (~/.local/bin may not be on PATH).
# `%u`: the link the app is opened for, if any.
sed "s|^Exec=.*|Exec=$bin %u|" acetate.desktop > "$entry"

# Tell the desktop about it (whichever of these exist).
command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications" || true
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" || true
# acetate:// links open in this app.
command -v xdg-mime >/dev/null && xdg-mime default acetate.desktop x-scheme-handler/acetate || true

echo "Installed:"
echo "  $bin"
echo "  $icon"
echo "  $entry"
echo "Start Acetate from your app menu (or run $bin)."
