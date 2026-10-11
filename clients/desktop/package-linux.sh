#!/bin/sh
# Builds the Linux app and packs it: clients/target/linux/Acetate-linux-x86_64.tar.gz, a folder
# with the program, its icon and menu entry, and install.sh (which installs the program
# that's there instead of building it). Music videos use the system's libmpv, as when
# built from the sources (README).
#
# Needs ALSA's headers (Fedora: alsa-lib-devel; Debian/Ubuntu: libasound2-dev). The
# release workflow runs this same script.
set -eu
cd "$(dirname "$0")"

out=../target/linux
app=$out/Acetate

cargo build --release

rm -rf "$app" "$out/Acetate-linux-x86_64.tar.gz"
mkdir -p "$app/assets"
install -m 755 ../target/release/acetate-desktop "$app/"
# Without the symbols (smaller).
strip "$app/acetate-desktop"
install -m 755 install.sh "$app/"
install -m 644 acetate.desktop "$app/"
install -m 644 assets/acetate-512.jpg ../assets/fonts/Inter-LICENSE.txt "$app/assets/"
tar -czf "$out/Acetate-linux-x86_64.tar.gz" -C "$out" Acetate
echo "Made $out/Acetate-linux-x86_64.tar.gz"
