#!/bin/sh
# Builds the Windows app from Linux and packs it: clients/target/windows/Acetate-windows-x86_64.zip,
# a folder with acetate-desktop.exe and mpv's library (libmpv-2.dll, for music videos)
# next to it. Unzip it anywhere and run the exe; Settings → Links → "Open links here"
# makes it what opens acetate:// links.
#
# Needs: the Rust target (rustup target add x86_64-pc-windows-gnu), MinGW (Fedora:
# mingw64-gcc mingw64-winpthreads-static; Debian/Ubuntu: mingw-w64), 7z (p7zip) and zip.
# The release workflow (.github/workflows/release.yml) runs this same script.
#
# libmpv is shinchiro's build (github.com/shinchiro/mpv-winbuild-cmake), one DLL with
# everything in it, pinned by its checksum; it's downloaded once into clients/target/windows.
set -eu
cd "$(dirname "$0")"

mpv_release=20261001
mpv_archive=mpv-dev-x86_64-20261001-git-3186d369f9.7z
mpv_sha256=de0aa24a39e27b07d9f663616f8d09c91586c7328d8c33fbfbc8e3888c35d89c

target=x86_64-pc-windows-gnu
out=../target/windows
app=$out/Acetate
mkdir -p "$out"

cargo build --release --target "$target"

if [ ! -f "$out/$mpv_archive" ]; then
    curl -fL -o "$out/$mpv_archive.part" "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/$mpv_release/$mpv_archive"
    mv "$out/$mpv_archive.part" "$out/$mpv_archive"
fi
echo "$mpv_sha256  $out/$mpv_archive" | sha256sum -c -

rm -rf "$app" "$out/Acetate-windows-x86_64.zip"
mkdir -p "$app"
cp "../target/$target/release/acetate-desktop.exe" "$app/Acetate.exe"
# Without the symbols (about 17 MB less).
x86_64-w64-mingw32-strip "$app/Acetate.exe"
7z e -y -bso0 -o"$app" "$out/$mpv_archive" libmpv-2.dll
cat > "$app/libmpv-LICENSE.txt" <<EOF
libmpv-2.dll is mpv (https://mpv.io), as built by https://github.com/shinchiro/mpv-winbuild-cmake
(release $mpv_release, $mpv_archive), with FFmpeg and the other libraries it includes.
It is licensed under the GNU GPL; the sources are at
https://github.com/mpv-player/mpv and listed by the build project above.
EOF
cp ../assets/fonts/Inter-LICENSE.txt "$app/"
(cd "$out" && zip -qr Acetate-windows-x86_64.zip Acetate)
echo "Made $out/Acetate-windows-x86_64.zip"
