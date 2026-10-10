#!/usr/bin/env bash
# Generates a handful of short, tagged test files in ./test-audio using ffmpeg. They are
# committed (the tests read them, and ffmpeg's output isn't byte for byte the same twice):
# this is how they were made, and how to make more. Other files in test-audio/ (your own
# music for a dev library) go in .git/info/exclude.
# Each "song" is a few seconds of sine wave at a different pitch.
set -euo pipefail

out="${1:-test-audio}"
mkdir -p "$out"
cd "$out"

tone() { # tone <freq> <seconds>
    echo "-f lavfi -i sine=frequency=$1:duration=$2:sample_rate=44100"
}

ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }

# A small cover image (solid colour square with a gradient).
ff -f lavfi -i "gradients=s=300x300:c0=0x3050a0:c1=0xe07040:duration=1" -frames:v 1 cover.png

# 1. FLAC with a cover, part of an album, a full timestamp as date, and an
#    ORIGINALFILENAME tag (which should win over the literal filename).
ff $(tone 440 4) -i cover.png -map 0:a -map 1:v -c:v png -disposition:v attached_pic \
    -metadata title="Glass Harbor" -metadata artist="Mira Vale" \
    -metadata album="Tidelines" -metadata album_artist="Mira Vale" \
    -metadata track="1/3" -metadata date="2021-06-04T11:11:11Z" -metadata genre="Dream Pop" \
    -metadata ORIGINALFILENAME="glass_harbor_MASTER_v7.wav" \
    "01 - Glass Harbor.flac"

# 2. FLAC, same album, with synced lyrics. The artist value is stored verbatim as ONE
#    credit ("Mira Vale feat. Oskar Lind"); nothing tries to split it.
ff $(tone 523 4) \
    -metadata title="Undertow" -metadata artist="Mira Vale feat. Oskar Lind" \
    -metadata album="Tidelines" -metadata album_artist="Mira Vale" \
    -metadata track="2/3" -metadata date="2021-06-04" -metadata genre="Dream Pop" \
    -metadata LYRICS="[00:00.50] Pulled beneath the surface
[00:02.00] Where the light gets thin" \
    "02 - Undertow.flac"

# 3. MP3 (ID3v2.4), same artist with different capitalization, timestamp date,
#    original filename in a TXXX frame.
ff $(tone 330 3) -c:a libmp3lame -b:a 192k -id3v2_version 4 \
    -metadata title="Lantern (Demo)" -metadata artist="mira vale" \
    -metadata date="2019-05-12T11:11:11" -metadata genre="Indie" \
    -metadata ORIGINALFILENAME="lantern idea 3.m4a" \
    -metadata lyrics="Hold the lantern high
Plain lyrics, not synced" \
    "lantern demo.mp3"

# 4. Low quality MP3 of a song by a different artist; see upgrades/ for the better version.
ff $(tone 660 3) -c:a libmp3lame -b:a 64k \
    -metadata title="Northbound" -metadata artist="Oskar Lind" \
    -metadata album="Northbound" -metadata date="2023-02" \
    "northbound (yt rip).mp3"

# 5. M4A (AAC).
ff $(tone 392 3) -c:a aac -b:a 160k \
    -metadata title="Salt & Static" -metadata artist="Simon & The Wires" \
    -metadata album="Salt & Static EP" -metadata track="1" -metadata date="2022-11-18" \
    "salt-and-static.m4a"

# 6. No tags at all: should be skipped (missing title and artist), not imported.
ff $(tone 250 2) -map_metadata -1 -fflags +bitexact -flags:a +bitexact "untitled_bounce_final_v3.flac"

# 7. An exact copy of #1 under a different name (should be detected as a duplicate).
cp "01 - Glass Harbor.flac" "glass harbor (copy).flac"

# 8. Opus with only a title: should be skipped (missing artist).
ff $(tone 800 2) -c:a libopus -metadata title="No Artist Here" "no-artist.opus"

# Upgrade for #4: a lossless version, for `acetate replace-file`.
mkdir -p upgrades
ff $(tone 660 3) -metadata title="Northbound" -metadata artist="Oskar Lind" \
    -metadata ORIGINALFILENAME="Northbound (Master).wav" \
    "upgrades/northbound.flac"

rm cover.png
echo "Wrote test files to $(pwd):"
ls -1R
