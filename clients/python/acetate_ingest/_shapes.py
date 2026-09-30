# Made from the server's source shapes (`SHAPES` in src/model/sources.rs): don't edit.
# `cargo test` checks it's up to date; `ACETATE_WRITE_PYTHON_SHAPES=1 cargo test python_shapes`
# makes it again.

from dataclasses import dataclass
from typing import ClassVar

from ._source import Field, Shape, ShapedSource

SHAPES: dict[tuple[str, str], Shape] = {
    ("soundcloud", "track"): Shape(id_field="song_id", fields={
        "song_id": Field("int", required=False),
        "song_uids": Field("strs", required=False),
        "song_permalinks": Field("strs", required=False),
        "song_descriptions": Field("strs", required=False),
        "release_dates": Field("strs", required=False),
        "created_at": Field("strs", required=False),
        "song_uid": Field("str", required=False),
        "song_permalink": Field("str", required=False),
        "song_description": Field("str", required=False),
    }),
    ("soundcloud", "album"): Shape(id_field="album_id", fields={
        "album_id": Field("int", required=False),
        "album_permalinks": Field("strs", required=False),
        "release_dates": Field("strs", required=False),
        "album_permalink": Field("str", required=False),
    }),
    ("soundcloud", "artist"): Shape(id_field="artist_id", fields={
        "artist_id": Field("int", required=False),
        "artist_permalinks": Field("strs", required=False),
        "artist_permalink": Field("str", required=False),
    }),
    ("distrokid", "track"): Shape(id_field=None, fields={
        "album_uuid": Field("str", required=True),
        "index": Field("int", required=False),
    }),
    ("distrokid", "album"): Shape(id_field="album_uuid", fields={
        "album_uuid": Field("str", required=True),
    }),
    ("distrokid", "artist"): Shape(id_field="artist_id", fields={
        "artist_id": Field("int", required=False),
        "artist_uuid": Field("str", required=False),
    }),
    ("netease", "track"): Shape(id_field="song_id", fields={
        "song_id": Field("int", required=True),
        "song_index": Field("int", required=False),
    }),
    ("netease", "album"): Shape(id_field="album_id", fields={
        "album_id": Field("int", required=True),
    }),
    ("netease", "artist"): Shape(id_field="artist_id", fields={
        "artist_id": Field("int", required=True),
    }),
    ("qq", "track"): Shape(id_field="song_id", fields={
        "song_id": Field("int", required=True),
        "song_mid": Field("str", required=False),
        "song_index": Field("int", required=False),
    }),
    ("qq", "album"): Shape(id_field="album_id", fields={
        "album_id": Field("int", required=True),
        "album_mid": Field("str", required=False),
    }),
    ("qq", "artist"): Shape(id_field="artist_id", fields={
        "artist_id": Field("int", required=True),
        "artist_mid": Field("str", required=False),
    }),
    ("kuwo", "track"): Shape(id_field="song_id", fields={
        "song_id": Field("int", required=False),
        "song_index": Field("int", required=False),
    }),
    ("kuwo", "album"): Shape(id_field="album_id", fields={
        "album_id": Field("int", required=True),
    }),
    ("kuwo", "artist"): Shape(id_field="artist_id", fields={
        "artist_id": Field("int", required=True),
    }),
    ("youtube", "track"): Shape(id_field="song_id", fields={
        "song_id": Field("str", required=True),
        "song_description": Field("str", required=False),
        "song_release_date": Field("str", required=False),
    }),
    ("youtube", "artist"): Shape(id_field="channel", fields={
        "channel": Field("str", required=True),
    }),
    ("traktrain", "track"): Shape(id_field="track_id", fields={
        "track_id": Field("int", required=True),
        "name": Field("str", required=False),
        "bpm": Field("int", required=False),
        "tags": Field("strs", required=False),
        "genre_ids": Field("strs", required=False),
        "audio_key": Field("str", required=False),
        "status": Field("str", required=False),
    }),
    ("traktrain", "artist"): Shape(id_field="user_id", fields={
        "user_id": Field("int", required=True),
        "slug": Field("str", required=False),
        "brand": Field("str", required=False),
    }),
    ("beatstars", "track"): Shape(id_field="track_id", fields={
        "track_id": Field("int", required=True),
    }),
    ("beatstars", "artist"): Shape(id_field="slug", fields={
        "slug": Field("str", required=True),
    }),
    ("custom", "track"): Shape(id_field=None, fields={
        "label": Field("str", required=True),
        "description": Field("str", required=False),
        "url": Field("str", required=False),
    }),
    ("custom", "album"): Shape(id_field=None, fields={
        "label": Field("str", required=True),
        "description": Field("str", required=False),
        "url": Field("str", required=False),
    }),
    ("custom", "artist"): Shape(id_field=None, fields={
        "label": Field("str", required=True),
        "description": Field("str", required=False),
        "url": Field("str", required=False),
    }),
}


@dataclass(frozen=True, kw_only=True)
class SoundCloudTrack(ShapedSource):
    """A track's source on soundcloud. Its id: `song_id`."""

    platform: ClassVar[str] = "soundcloud"
    kind: ClassVar[str] = "track"
    song_id: int | None = None
    song_uids: list[str] | None = None
    song_permalinks: list[str] | None = None
    song_descriptions: list[str] | None = None
    release_dates: list[str] | None = None
    created_at: list[str] | None = None
    song_uid: str | None = None
    song_permalink: str | None = None
    song_description: str | None = None


@dataclass(frozen=True, kw_only=True)
class SoundCloudAlbum(ShapedSource):
    """A album's source on soundcloud. Its id: `album_id`."""

    platform: ClassVar[str] = "soundcloud"
    kind: ClassVar[str] = "album"
    album_id: int | None = None
    album_permalinks: list[str] | None = None
    release_dates: list[str] | None = None
    album_permalink: str | None = None


@dataclass(frozen=True, kw_only=True)
class SoundCloudArtist(ShapedSource):
    """A artist's source on soundcloud. Its id: `artist_id`."""

    platform: ClassVar[str] = "soundcloud"
    kind: ClassVar[str] = "artist"
    artist_id: int | None = None
    artist_permalinks: list[str] | None = None
    artist_permalink: str | None = None


@dataclass(frozen=True, kw_only=True)
class DistroKidTrack(ShapedSource):
    """A track's source on distrokid. Its id: none: sources with the same data are the same."""

    platform: ClassVar[str] = "distrokid"
    kind: ClassVar[str] = "track"
    album_uuid: str
    index: int | None = None


@dataclass(frozen=True, kw_only=True)
class DistroKidAlbum(ShapedSource):
    """A album's source on distrokid. Its id: `album_uuid`."""

    platform: ClassVar[str] = "distrokid"
    kind: ClassVar[str] = "album"
    album_uuid: str


@dataclass(frozen=True, kw_only=True)
class DistroKidArtist(ShapedSource):
    """A artist's source on distrokid. Its id: `artist_id`."""

    platform: ClassVar[str] = "distrokid"
    kind: ClassVar[str] = "artist"
    artist_id: int | None = None
    artist_uuid: str | None = None


@dataclass(frozen=True, kw_only=True)
class NetEaseTrack(ShapedSource):
    """A track's source on netease. Its id: `song_id`."""

    platform: ClassVar[str] = "netease"
    kind: ClassVar[str] = "track"
    song_id: int
    song_index: int | None = None


@dataclass(frozen=True, kw_only=True)
class NetEaseAlbum(ShapedSource):
    """A album's source on netease. Its id: `album_id`."""

    platform: ClassVar[str] = "netease"
    kind: ClassVar[str] = "album"
    album_id: int


@dataclass(frozen=True, kw_only=True)
class NetEaseArtist(ShapedSource):
    """A artist's source on netease. Its id: `artist_id`."""

    platform: ClassVar[str] = "netease"
    kind: ClassVar[str] = "artist"
    artist_id: int


@dataclass(frozen=True, kw_only=True)
class QQTrack(ShapedSource):
    """A track's source on qq. Its id: `song_id`."""

    platform: ClassVar[str] = "qq"
    kind: ClassVar[str] = "track"
    song_id: int
    song_mid: str | None = None
    song_index: int | None = None


@dataclass(frozen=True, kw_only=True)
class QQAlbum(ShapedSource):
    """A album's source on qq. Its id: `album_id`."""

    platform: ClassVar[str] = "qq"
    kind: ClassVar[str] = "album"
    album_id: int
    album_mid: str | None = None


@dataclass(frozen=True, kw_only=True)
class QQArtist(ShapedSource):
    """A artist's source on qq. Its id: `artist_id`."""

    platform: ClassVar[str] = "qq"
    kind: ClassVar[str] = "artist"
    artist_id: int
    artist_mid: str | None = None


@dataclass(frozen=True, kw_only=True)
class KuwoTrack(ShapedSource):
    """A track's source on kuwo. Its id: `song_id`."""

    platform: ClassVar[str] = "kuwo"
    kind: ClassVar[str] = "track"
    song_id: int | None = None
    song_index: int | None = None


@dataclass(frozen=True, kw_only=True)
class KuwoAlbum(ShapedSource):
    """A album's source on kuwo. Its id: `album_id`."""

    platform: ClassVar[str] = "kuwo"
    kind: ClassVar[str] = "album"
    album_id: int


@dataclass(frozen=True, kw_only=True)
class KuwoArtist(ShapedSource):
    """A artist's source on kuwo. Its id: `artist_id`."""

    platform: ClassVar[str] = "kuwo"
    kind: ClassVar[str] = "artist"
    artist_id: int


@dataclass(frozen=True, kw_only=True)
class YouTubeTrack(ShapedSource):
    """A track's source on youtube. Its id: `song_id`."""

    platform: ClassVar[str] = "youtube"
    kind: ClassVar[str] = "track"
    song_id: str
    song_description: str | None = None
    song_release_date: str | None = None


@dataclass(frozen=True, kw_only=True)
class YouTubeArtist(ShapedSource):
    """A artist's source on youtube. Its id: `channel`."""

    platform: ClassVar[str] = "youtube"
    kind: ClassVar[str] = "artist"
    channel: str


@dataclass(frozen=True, kw_only=True)
class TraktrainTrack(ShapedSource):
    """A track's source on traktrain. Its id: `track_id`."""

    platform: ClassVar[str] = "traktrain"
    kind: ClassVar[str] = "track"
    track_id: int
    name: str | None = None
    bpm: int | None = None
    tags: list[str] | None = None
    genre_ids: list[str] | None = None
    audio_key: str | None = None
    status: str | None = None


@dataclass(frozen=True, kw_only=True)
class TraktrainArtist(ShapedSource):
    """A artist's source on traktrain. Its id: `user_id`."""

    platform: ClassVar[str] = "traktrain"
    kind: ClassVar[str] = "artist"
    user_id: int
    slug: str | None = None
    brand: str | None = None


@dataclass(frozen=True, kw_only=True)
class BeatStarsTrack(ShapedSource):
    """A track's source on beatstars. Its id: `track_id`."""

    platform: ClassVar[str] = "beatstars"
    kind: ClassVar[str] = "track"
    track_id: int


@dataclass(frozen=True, kw_only=True)
class BeatStarsArtist(ShapedSource):
    """A artist's source on beatstars. Its id: `slug`."""

    platform: ClassVar[str] = "beatstars"
    kind: ClassVar[str] = "artist"
    slug: str


@dataclass(frozen=True, kw_only=True)
class CustomTrack(ShapedSource):
    """A track's source on custom. Its id: none: sources with the same data are the same."""

    platform: ClassVar[str] = "custom"
    kind: ClassVar[str] = "track"
    label: str
    description: str | None = None
    url: str | None = None


@dataclass(frozen=True, kw_only=True)
class CustomAlbum(ShapedSource):
    """A album's source on custom. Its id: none: sources with the same data are the same."""

    platform: ClassVar[str] = "custom"
    kind: ClassVar[str] = "album"
    label: str
    description: str | None = None
    url: str | None = None


@dataclass(frozen=True, kw_only=True)
class CustomArtist(ShapedSource):
    """A artist's source on custom. Its id: none: sources with the same data are the same."""

    platform: ClassVar[str] = "custom"
    kind: ClassVar[str] = "artist"
    label: str
    description: str | None = None
    url: str | None = None


__all__ = [
    "SoundCloudTrack",
    "SoundCloudAlbum",
    "SoundCloudArtist",
    "DistroKidTrack",
    "DistroKidAlbum",
    "DistroKidArtist",
    "NetEaseTrack",
    "NetEaseAlbum",
    "NetEaseArtist",
    "QQTrack",
    "QQAlbum",
    "QQArtist",
    "KuwoTrack",
    "KuwoAlbum",
    "KuwoArtist",
    "YouTubeTrack",
    "YouTubeArtist",
    "TraktrainTrack",
    "TraktrainArtist",
    "BeatStarsTrack",
    "BeatStarsArtist",
    "CustomTrack",
    "CustomAlbum",
    "CustomArtist",
]
