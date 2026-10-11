#!/usr/bin/env python3
"""YouTube's part of Acetate that needs code (see docs/admin/extensions.md): a request as one
JSON object on stdin, an answer as one on stdout.

  {"op": "metadata", "input": "<link or id>", "match": {"song_id": "<the video's id>"}}
      Reads the video with yt-dlp.
  {"op": "parse", "input": {<a yt-dlp info JSON, pasted>}}
      Reads what was pasted.

The answer is the track's metadata (title, date, artists, the uploader's account, the source,
the artwork), or {"error": "why"} (with "unavailable": true when yt-dlp isn't installed).
Environment: ACETATE_YTDLP_COOKIES, a cookies.txt (Netscape format) of a logged-in browser,
for "Sign in to confirm you're not a bot"; ACETATE_FETCH_TIMEOUT, the seconds it has.
"""

import json
import os
import subprocess
import sys

#: What yt-dlp's JSON has that no one here reads, and that makes it big.
LEFT_OUT = ["formats", "requested_formats", "requested_downloads", "thumbnails", "automatic_captions", "subtitles", "heatmap", "_format_sort_fields"]


def text(info, key):
    value = info.get(key)
    return value.strip() if isinstance(value, str) and value.strip() else None


def names(info, key):
    value = info.get(key)
    if not isinstance(value, list):
        return []
    return [v.strip() for v in value if isinstance(v, str) and v.strip()]


def one(info, key):
    value = text(info, key)
    return [value] if value else []


def iso(date):
    return f"{date[:4]}-{date[4:6]}-{date[6:]}" if len(date) == 8 and date.isdigit() else date


def normalize(info):
    """The track's metadata from a video's yt-dlp JSON. The artist: yt-dlp's music fields
    (`artists` / `artist`, `creators` / `creator`), else the channel (whose account it is
    then: matched by its handle or id, stored as its id). The title: `track`, else the
    video's."""
    video_id = text(info, "id")
    if not video_id:
        raise ValueError("the yt-dlp JSON has no video id")
    title = text(info, "track") or text(info, "title")
    if not title:
        raise ValueError("the yt-dlp JSON has no title")
    music = next((l for l in (names(info, "artists"), one(info, "artist"), names(info, "creators"), one(info, "creator")) if l), None)
    account = None
    if music:
        artists = music
    else:
        channel = text(info, "channel") or text(info, "uploader")
        if not channel:
            raise ValueError("the yt-dlp JSON says no artist and no channel")
        artists = [channel]
        channel_id = text(info, "channel_id")
        handle = (text(info, "uploader_id") or "").lstrip("@") or None
        stored = channel_id or handle
        if stored:
            account = {
                "platform": "youtube",
                "ids": [i for i in (channel_id, handle) if i],
                "label": f"youtube @{handle}" if handle else f"youtube {stored}",
                "data": {"channel": stored},
            }
    uploaded = text(info, "upload_date")
    uploaded = iso(uploaded) if uploaded else None
    released = text(info, "release_date")
    date = iso(released) if released else uploaded
    data = {"song_id": video_id}
    if text(info, "description"):
        data["song_description"] = text(info, "description")
    if uploaded:
        data["song_release_date"] = uploaded
    thumbnail = text(info, "thumbnail")
    return {
        "from": "YouTube",
        "title": title,
        "date": date,
        "genre": None,
        "artists": artists,
        "account": account,
        "source": {"platform": "youtube", "id": video_id, "data": data},
        "artwork": [thumbnail] if thumbnail else [],
    }


def read_video(video_id):
    """The video's yt-dlp JSON."""
    seconds = max(int(os.environ.get("ACETATE_FETCH_TIMEOUT", "60")) - 5, 5)
    command = ["yt-dlp", "-J", "--no-playlist", "--skip-download", "--no-warnings", "--ignore-no-formats-error"]
    cookies = os.environ.get("ACETATE_YTDLP_COOKIES")
    if cookies:
        command += ["--cookies", cookies]
    command += ["--", f"https://www.youtube.com/watch?v={video_id}"]
    try:
        done = subprocess.run(command, capture_output=True, text=True, stdin=subprocess.DEVNULL, timeout=seconds)
    except FileNotFoundError:
        raise Unavailable("yt-dlp isn't installed on the server: paste the video's yt-dlp JSON instead") from None
    except subprocess.TimeoutExpired:
        raise ValueError(f"yt-dlp took longer than {seconds} seconds") from None
    if done.returncode != 0:
        lines = [l for l in done.stderr.splitlines() if l.strip()]
        why = (lines[-1].strip() if lines else "it failed").removeprefix("ERROR: ")
        hint = " (set ACETATE_YTDLP_COOKIES to a cookies.txt file from a logged-in browser)" if "Sign in" in why else ""
        raise ValueError(f"yt-dlp couldn't read the video: {why}{hint}")
    try:
        info = json.loads(done.stdout)
    except ValueError as err:
        raise ValueError(f"yt-dlp's answer isn't JSON: {err}") from None
    for key in LEFT_OUT:
        info.pop(key, None)
    return info


class Unavailable(Exception):
    pass


def answer(request):
    op = request.get("op")
    if op == "metadata":
        video_id = (request.get("match") or {}).get("song_id")
        if not video_id:
            raise ValueError("not a YouTube video's link or id")
        info = read_video(video_id)
        metadata = normalize(info)
        if metadata["source"]["id"] != video_id:
            raise ValueError(f"YouTube answered for another video ({metadata['source']['id']})")
        return metadata
    if op == "parse":
        info = request.get("input")
        if not isinstance(info, dict):
            raise ValueError("not a yt-dlp JSON")
        extractor = (text(info, "extractor_key") or "")
        if extractor and extractor.lower() != "youtube":
            raise ValueError(f"a yt-dlp JSON from {extractor}: only YouTube's is read (for SoundCloud, paste the SoundCloud track's own JSON)")
        return normalize(info)
    raise ValueError(f"no such operation: {op}")


def main():
    try:
        print(json.dumps(answer(json.load(sys.stdin))))
    except Unavailable as err:
        print(json.dumps({"error": str(err), "unavailable": True}))
    except ValueError as err:
        print(json.dumps({"error": str(err)}))


if __name__ == "__main__":
    main()
