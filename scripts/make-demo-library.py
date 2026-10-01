#!/usr/bin/env python3
"""Generates an invented library for screenshots and demos: ~1000 tagged Opus files in
./demo-audio, by made-up artists, with gradient covers, synced lyrics on some, features,
producers and original filenames. Audio is a few minutes of synthetic chords and a kick,
enough for the visualizer. The same seed gives the same library.

    scripts/make-demo-library.py [--tracks 1000] [--out demo-audio] [--seed 7]

Import it with `acetate ingest demo-audio --layout tags --origin demo` (see
scripts/reset-demo-db.sh). Needs ffmpeg (with libopus) and ImageMagick.
"""

import argparse
import base64
import colorsys
import os
import random
import struct
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

FIRST = ["Mira", "Oskar", "Juno", "Theo", "Lumi", "Sable", "Ezra", "Noor", "Kai", "Ilse",
         "Rafa", "Wren", "Otto", "Selma", "Dario", "Aino", "Cass", "Felix", "Lior", "Maren"]
LAST = ["Vale", "Lind", "Hart", "Okoye", "Strand", "Moreau", "Kask", "Reyes", "Holm",
        "Achebe", "Novak", "Sato", "Brandt", "Quill", "Ferro", "Lund", "Ash", "Pike"]
BAND_A = ["Velvet", "Paper", "Static", "Hollow", "Neon", "Glass", "Quiet", "Copper",
          "Midnight", "Salt", "Lunar", "Wild", "Silver", "Low", "Cold"]
BAND_B = ["Harbor", "Engines", "Orchard", "Choir", "Arcade", "Lanterns", "Tides",
          "Machines", "Gardens", "Signals", "Wolves", "Rooms", "Satellites", "Ferns"]
SOLO = ["DJ Kestrel", "lowmoth", "SUNDOG", "yesterhaze", "Pale Atlas", "kiddo static",
        "Marlowe", "ØRBIT", "fennel", "Tamsin K", "GHOSTWAVE", "noa noa", "Halcyon Drip"]
PRODUCERS = ["Wrenfield", "Bastian Cole", "808 Ferro", "Lumen", "pk.beats", "Nils Arvo"]

WORDS = ["Light", "Tide", "Static", "Northbound", "Ember", "Hollow", "Satellite", "Paper",
         "River", "Glass", "Fever", "Velvet", "Harbor", "Lantern", "Signal", "Orbit",
         "Echo", "Winter", "Gold", "Ghost", "Summer", "Midnight", "Undertow", "Bloom",
         "Wires", "Neon", "Silence", "Heartbeat", "Avenue", "Machine", "Ocean", "Dust",
         "Halo", "Mirror", "Comet", "Thunder", "Honey", "Shadow", "Daylight", "Salt"]
PATTERNS = ["{a}", "{a} {b}", "The {a}", "{a} & {b}", "Into the {a}", "{a} Song",
            "No More {a}", "{a} ({b} Mix)", "Last {a}", "Under {a}", "{a}lines", "{a}!"]
GENRES = ["Indie Pop", "Dream Pop", "Synthwave", "Hip Hop", "Trap", "Lo-fi", "Shoegaze",
          "House", "Ambient", "Alt Rock", "R&B", "Electronic", "Folk", "Drum & Bass"]
LYRIC_LINES = ["Pulled beneath the surface", "Where the light gets thin", "Hold the lantern high",
               "We were running out of summer", "Every signal fades to grey",
               "Call me when the city sleeps", "I kept your picture in the glass",
               "Northbound on an empty road", "Nothing here but static now",
               "Say it like you mean it", "Falling slow, falling slow", "Ghosts in the machine",
               "Count the satellites with me", "Honey, it's later than you think",
               "All the rivers lead to you", "Dancing in the afterglow"]

NOTES = [0, 2, 3, 5, 7, 8, 10]


def title(rng):
    a, b = rng.sample(WORDS, 2)
    return rng.choice(PATTERNS).format(a=a, b=b.lower() if rng.random() < .3 else b)


def make_artists(rng):
    names = set()
    while len(names) < 45:
        r = rng.random()
        if r < .45:
            names.add(f"{rng.choice(FIRST)} {rng.choice(LAST)}")
        elif r < .8:
            names.add(f"{rng.choice(BAND_A)} {rng.choice(BAND_B)}")
        else:
            names.add(rng.choice(SOLO))
    return [{"name": n, "genre": rng.choice(GENRES), "start": rng.randint(2006, 2020)}
            for n in sorted(names)]


def make_library(rng, total):
    artists = make_artists(rng)
    albums, count = [], 0
    while count < total:
        artist = rng.choice(artists)
        kind = rng.choices(["album", "ep", "single"], [5, 3, 3])[0]
        n = {"album": rng.randint(8, 14), "ep": rng.randint(4, 6), "single": rng.randint(1, 2)}[kind]
        n = min(n, total - count)
        tracks = [title(rng) for _ in range(n)]
        name = tracks[0] if kind == "single" else title(rng) + (" EP" if kind == "ep" else "")
        year = min(2025, artist["start"] + rng.randint(0, 6))
        albums.append({"artist": artist, "name": name, "year": year,
                       "date": f"{year}-{rng.randint(1, 12):02}-{rng.randint(1, 28):02}",
                       "tracks": tracks, "colors": palette(rng),
                       "style": rng.choice(["linear", "radial", "plasma"]), "seed": rng.randrange(1 << 30)})
        count += n
    return artists, albums


def palette(rng):
    """Two colours that go together: hues apart, softer saturation."""
    h = rng.random()
    out = []
    for hue, light in ((h, rng.uniform(.25, .45)), (h + rng.uniform(.08, .4), rng.uniform(.55, .75))):
        r, g, b = colorsys.hls_to_rgb(hue % 1, light, rng.uniform(.45, .8))
        out.append("#%02x%02x%02x" % (int(r * 255), int(g * 255), int(b * 255)))
    return out


def cover(album, path):
    a, b = album["colors"]
    size = "600x600"
    if album["style"] == "linear":
        # Rotated in place, the corners filled with the nearest edge's colour.
        args = ["-size", size, f"gradient:{a}-{b}", "-virtual-pixel", "edge",
                "-distort", "SRT", str(album["seed"] % 360)]
    elif album["style"] == "radial":
        args = ["-size", size, f"radial-gradient:{b}-{a}"]
    else:
        # Soft blobs: plasma, blurred until only the colour fields are left.
        args = ["-seed", str(album["seed"]), "-size", "150x150", f"plasma:{a}-{b}",
                "-blur", "0x12", "-resize", size]
    subprocess.run(["magick", *args, "-quality", "85", path], check=True)


def picture_block(jpeg):
    """A FLAC picture block, base64: how Vorbis comments (Opus) carry cover art."""
    def text(s):
        return struct.pack(">I", len(s)) + s
    block = struct.pack(">I", 3) + text(b"image/jpeg") + text(b"") \
        + struct.pack(">IIII", 600, 600, 24, 0) + text(jpeg)
    return base64.b64encode(block).decode()


def lyrics(rng, seconds, artists):
    lines, t = [], 4.0
    for part in ["Verse 1", "Chorus", "Verse 2", "Chorus", "Bridge", "Chorus"]:
        if t > seconds - 10:
            break
        who = f": {rng.choice(artists)}" if len(artists) > 1 and part.startswith("Verse") else ""
        lines.append(f"[{int(t // 60):02}:{t % 60:05.2f}][{part}{who}]")
        for _ in range(4):
            lines.append(f"[{int(t // 60):02}:{t % 60:05.2f}]{rng.choice(LYRIC_LINES)}")
            t += rng.uniform(3, 5)
        t += rng.uniform(2, 8)
    return "\n".join(lines)


def audio_expr(rng, bpm):
    root = 110 * 2 ** (rng.randrange(12) / 12)
    prog = rng.sample(NOTES, 4)
    bar = 240 / bpm
    beat = bar / 4
    n = lambda i: f"{root}*pow(2,{prog[i]}/12)"
    f = f"if(lt(mod(t,{4 * bar}),{bar}),{n(0)},if(lt(mod(t,{4 * bar}),{2 * bar}),{n(1)},if(lt(mod(t,{4 * bar}),{3 * bar}),{n(2)},{n(3)})))"
    pulse = f"(0.55+0.45*exp(-mod(t,{beat / 2})*6))"
    chord = f"(sin(2*PI*{f}*t)+0.6*sin(2*PI*{f}*1.26*t)+0.5*sin(2*PI*{f}*1.5*t)+0.3*sin(2*PI*{f}*2*t))"
    kick = f"0.9*sin(2*PI*(45+90*exp(-mod(t,{beat})*30))*t)*exp(-mod(t,{beat})*9)"
    hat = f"0.08*(random(0)*2-1)*exp(-mod(t+{beat / 2},{beat})*40)"
    return f"0.12*{chord}*{pulse}+0.35*{kick}+{hat}"


def encode(job):
    path, seconds, expr, tags = job
    if os.path.exists(path):
        return
    args = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi",
            "-i", f"aevalsrc='{expr}':s=24000:d={seconds}",
            "-af", f"afade=t=in:d=1,afade=t=out:st={seconds - 3}:d=3",
            "-c:a", "libopus", "-b:a", "16k", "-ac", "1"]
    for key, value in tags.items():
        args += ["-metadata", f"{key}={value}"]
    tmp = path + ".part.opus"
    subprocess.run([*args, tmp], check=True)
    os.replace(tmp, path)


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--tracks", type=int, default=1000)
    p.add_argument("--out", default="demo-audio")
    p.add_argument("--seed", type=int, default=7)
    opts = p.parse_args()

    rng = random.Random(opts.seed)
    artists, albums = make_library(rng, opts.tracks)
    names = [a["name"] for a in artists]
    os.makedirs(os.path.join(opts.out, ".covers"), exist_ok=True)

    jobs = []
    for i, album in enumerate(albums):
        jpg = os.path.join(opts.out, ".covers", f"{i:04}.jpg")
        if not os.path.exists(jpg):
            cover(album, jpg)
        with open(jpg, "rb") as fh:
            pic = picture_block(fh.read())
        folder = os.path.join(opts.out, album["artist"]["name"].replace("/", "_"),
                              f"{album['year']} - {album['name']}".replace("/", "_"))
        os.makedirs(folder, exist_ok=True)
        for n, name in enumerate(album["tracks"], 1):
            credits = [album["artist"]["name"]]
            if rng.random() < .15:
                credits.append(rng.choice([x for x in names if x not in credits]))
            seconds = rng.randint(110, 290)
            tags = {"title": name, "artist": ";".join(credits), "album": album["name"],
                    "album_artist": album["artist"]["name"], "track": f"{n}/{len(album['tracks'])}",
                    "date": album["date"], "genre": album["artist"]["genre"],
                    "METADATA_BLOCK_PICTURE": pic}
            if rng.random() < .3:
                tags["composer"] = rng.choice(PRODUCERS)
            if rng.random() < .2:
                tags["lyrics"] = lyrics(rng, seconds, credits)
            if rng.random() < .1:
                tags["ORIGINALFILENAME"] = f"{name.lower().replace(' ', '_')}_master_v{rng.randint(2, 9)}.wav"
            path = os.path.join(folder, f"{n:02} - {name}.opus".replace("/", "_"))
            jobs.append((path, seconds, audio_expr(rng, rng.choice([84, 92, 100, 120, 128, 140, 170])), tags))

    done = 0
    with ThreadPoolExecutor(max(1, (os.cpu_count() or 2) - 2)) as pool:
        for _ in pool.map(encode, jobs):
            done += 1
            if done % 50 == 0 or done == len(jobs):
                print(f"[{done}/{len(jobs)}]", file=sys.stderr)
    print(f"{len(jobs)} tracks, {len(albums)} albums, {len(artists)} artists in {opts.out}/")


if __name__ == "__main__":
    main()
