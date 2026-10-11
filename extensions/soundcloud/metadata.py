#!/usr/bin/env python3
"""SoundCloud's part of Acetate that needs code (see docs/admin/extensions.md): reading a
SoundCloud track's own JSON (the API's, a downloader's), pasted into the track editor.

  {"op": "parse", "input": {<the track's JSON>}}

The answer is the track's metadata (title, date, genre, artists, the uploader's account, the
source, the artwork), or {"error": "why"}.

The source's fields:
* `song_id`: `id`;
* `song_uids`: the uploaded file's id, from the waveform (`waveform_url`'s last part before
  `_`, else `waveformKey`'s); refused without one;
* `song_permalinks`: `permalink_url` after "soundcloud.com/";
* `release_dates`: `display_date`, else `created_at`; `created_at`;
* `song_descriptions`: `description`, if it has one.
"""

import json
import sys


def text(info, key):
    value = info.get(key)
    return value.strip() if isinstance(value, str) and value.strip() else None


def whole(value):
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def dedup(items):
    return list(dict.fromkeys(items))


def artwork_links(url):
    """A SoundCloud artwork's links, best first: its original (a JPEG, else a PNG: it's kept
    as uploaded), whatever size `url` is ("…-large.jpg", "…-t500x500.jpg"), then `url`."""
    path, _, query = url.partition("?")
    links = []
    name_at = path.rfind("/") + 1
    dash = path[name_at:].rfind("-")
    if dash >= 0:
        base = path[: name_at + dash]
        links += [f"{base}-original.jpg", f"{base}-original.png"]
    links.append(f"{path}?{query}" if query else path)
    return dedup(links)


def parse(info):
    song_id = whole(info.get("id"))
    if song_id is None:
        raise ValueError("the SoundCloud JSON has no track id (`id`)")
    if "waveform_url" in info:
        url = info["waveform_url"]
        if not isinstance(url, str) or not url:
            raise ValueError("couldn't tell the song's waveform: its waveform_url is null")
        uid = url.rsplit("/", 1)[-1].split("_")[0]
    elif "waveformKey" in info:
        key = info["waveformKey"]
        uid = key.split("_")[0] if isinstance(key, str) else ""
    else:
        raise ValueError("couldn't tell the song's waveform (no waveform_url or waveformKey)")
    if not uid:
        raise ValueError("couldn't tell the song's waveform: it's empty")
    title = text(info, "title")
    if not title:
        raise ValueError("the SoundCloud JSON has no title")
    created = text(info, "created_at")
    date = text(info, "display_date") or created
    permalink = None
    link = text(info, "permalink_url")
    if link and "soundcloud.com/" in link:
        permalink = link.split("soundcloud.com/", 1)[1].rstrip("/") or None

    data = {"song_id": song_id, "song_uids": [uid]}
    if permalink:
        data["song_permalinks"] = [permalink]
    if date:
        data["release_dates"] = [date]
    if created:
        data["created_at"] = [created]
    if text(info, "description"):
        data["song_descriptions"] = [text(info, "description")]

    # The artist: the publisher's (labels, distributors), else the uploader.
    user = info.get("user") if isinstance(info.get("user"), dict) else {}
    uploader = text(user, "username")
    publisher_info = info.get("publisher_metadata")
    publisher = text(publisher_info, "artist") if isinstance(publisher_info, dict) else None
    artist = publisher or uploader
    if not artist:
        raise ValueError("the SoundCloud JSON says no artist (no publisher artist, no uploader)")
    is_uploader = uploader is not None and uploader.lower() == artist.lower()
    user_id = whole(user.get("id"))
    account = None
    if is_uploader and user_id is not None:
        handle = text(user, "permalink")
        account_data = {"artist_id": user_id}
        if handle:
            account_data["artist_permalinks"] = [handle]
        account = {"platform": "soundcloud", "ids": [str(user_id)], "label": f"soundcloud {handle or user_id}", "data": account_data}
    artwork_url = text(info, "artwork_url")
    return {
        "from": "SoundCloud",
        "title": title,
        "date": date,
        "genre": text(info, "genre"),
        "artists": [artist],
        "account": account,
        "source": {"platform": "soundcloud", "id": str(song_id), "data": data},
        "artwork": dedup(artwork_links(artwork_url)) if artwork_url else [],
    }


def answer(request):
    if request.get("op") != "parse":
        raise ValueError(f"no such operation: {request.get('op')}")
    info = request.get("input")
    if not isinstance(info, dict):
        raise ValueError("not a SoundCloud track's JSON")
    return parse(info)


def main():
    try:
        print(json.dumps(answer(json.load(sys.stdin))))
    except ValueError as err:
        print(json.dumps({"error": str(err)}))


if __name__ == "__main__":
    main()
