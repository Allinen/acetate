"""A description: what a client knows about a file beyond its tags (`POST /api/ingest`).

Two ways:

* **From the tags** (without `metadata`): the track's metadata comes from the file's tags.
  The description says where the file came from (`origin`), which artists and album the
  names in the tags are (by their ids on a platform, or by Acetate id), and adds covers and
  a few facts the tags can't hold.
* **All given** (with `metadata`): the description has all of it (title, credits with
  roles and ids, album, date, lyrics...), and the file's tags are used for nothing. For
  files straight from a platform, without tags or with tags that can't be trusted. The tags
  are still kept with the file, as they are.

See the README's "Automated ingestion" for how the server uses each part.
"""

from __future__ import annotations

import os
import uuid
from dataclasses import dataclass, field
from typing import Any, Callable, Union

from ._shapes import SHAPES
from ._source import ALBUM, ARTIST, TRACK, Source, check_uuid
from .errors import InvalidDescription

#: An image: its bytes, or the path of a file holding them.
Image = Union[bytes, str, "os.PathLike[str]"]


@dataclass(kw_only=True)
class TrackHint:
    """About the track itself."""

    #: Other places the song can be found (the origin is one already).
    sources: list[Source] = field(default_factory=list)
    #: Images added as the track's covers, after the file's embedded pictures.
    covers: list[Image] = field(default_factory=list)
    #: Leave out the file's embedded pictures (e.g. when they're smaller copies of `covers`).
    skip_embedded_covers: bool = False
    #: Used if the tags have no date (or over theirs, with `release_date_over_tags`).
    #: Verbatim: any text, never reformatted.
    release_date: str | None = None
    release_date_over_tags: bool = False
    #: It's an original file (as the artist named it), with this name, when its tags don't
    #: say so. Only for real original files: never a download's own name.
    original_filename: str | None = None


@dataclass(kw_only=True)
class AlbumHint:
    """About the album in the file's ALBUM tag."""

    #: "It's this Acetate album": no matching at all.
    id: str | uuid.UUID | None = None
    #: The album's ids on platforms: it's found by them, and they're added to it.
    sources: list[Source] = field(default_factory=list)
    #: Images added as the album's covers (skipped where the album has that image already).
    covers: list[Image] = field(default_factory=list)
    #: The track's position on the album, instead of the TRACK tag's.
    track_number: int | None = None
    #: A single (True) or an album (False). None: the tags decide (a track count of 1 is a
    #: single).
    single: bool | None = None
    #: The album's own release date, when it isn't the track's.
    release_date: str | None = None


@dataclass
class ArtistHint:
    """About one artist named in the ARTIST or ALBUM ARTIST tags."""

    #: The name exactly as in the tags (after splitting on `;`).
    name: str
    #: "It's this Acetate artist": no matching at all.
    id: str | uuid.UUID | None = field(default=None, kw_only=True)
    #: The artist's ids on platforms: it's found by them, and they're added to it.
    sources: list[Source] = field(default_factory=list, kw_only=True)


@dataclass(kw_only=True)
class Given:
    """Values from outside the tags (a file name, a platform's lookup), used where the
    tags have none. The first revision says where each came from (`*_from`)."""

    title: str | None = None
    title_from: str = ""
    artists: list[str] = field(default_factory=list)
    artists_from: str = ""
    #: The given artists are the track's (and its album's), whatever the tags say.
    artists_over_tags: bool = False


@dataclass
class Credit:
    """An artist credited on the track (or the album), with `metadata`."""

    #: The name it's credited under, as it should show (the artist's name, or one of its
    #: other names).
    name: str
    #: "primary", "featured", "producer" (the server's roles: `GET /api/vocab`).
    role: str = "primary"
    #: "It's this Acetate artist": no matching at all.
    id: str | uuid.UUID | None = field(default=None, kw_only=True)
    #: The artist's ids on platforms: it's found by them (else by the name), and they're
    #: added to it.
    sources: list[Source] = field(default_factory=list, kw_only=True)


@dataclass(kw_only=True)
class Album:
    """The album the track is on, with `metadata`."""

    title: str
    #: "album", "single", "ep", "compilation", "mixtape".
    kind: str = "album"
    #: The album's own credits. Empty: the track's primary artists.
    artists: list[Credit] = field(default_factory=list)
    release_date: str | None = None
    disc_number: int | None = None
    track_number: int | None = None
    #: "It's this Acetate album": no matching at all.
    id: str | uuid.UUID | None = None
    #: The album's ids on platforms: it's found by them (else by title, artists and date).
    sources: list[Source] = field(default_factory=list)
    covers: list[Image] = field(default_factory=list)


@dataclass
class Lyrics:
    #: Plain text, or LRC (timed lines) as it is.
    body: str
    #: From the artist or label (not transcribed by someone).
    official: bool = field(default=False, kw_only=True)
    #: "en", "ja"...
    language: str | None = field(default=None, kw_only=True)


@dataclass(kw_only=True)
class Metadata:
    """All of the track's metadata, when the file's tags aren't to be used (a file straight
    from a platform: no tags, or tags that can't be trusted). The file's tags are then used
    for nothing (not even its pictures or lyrics); they're kept with the file as they are."""

    title: str
    #: In order; at least one primary.
    credits: list[Credit]
    #: Verbatim: any text, never reformatted.
    release_date: str | None = None
    genres: list[str] = field(default_factory=list)
    comment: str | None = None
    album: Album | None = None
    lyrics: list[Lyrics] = field(default_factory=list)


@dataclass(kw_only=True)
class Description:
    """Everything sent with one file. Only `origin` is required."""

    #: Where the file came from: a track source ("this file is that NetEase song", or a
    #: custom source like "sent on Discord").
    origin: Source
    #: The track's status (`released`, `unreleased`, `demo`...; the server's list is in
    #: `GET /api/vocab`). None: released.
    status: str | None = None
    track: TrackHint = field(default_factory=TrackHint)
    album: AlbumHint | None = None
    artists: list[ArtistHint] = field(default_factory=list)
    #: Ids for the first artist in the ARTIST tag, whatever its name (e.g. the uploader of
    #: a SoundCloud song).
    first_artist_sources: list[Source] = field(default_factory=list)
    #: Create the track restricted to these groups (ids; `Acetate.group_ids` finds them by
    #: name). Needs the `manage_access` permission.
    restrict_to: list[str | uuid.UUID] | None = None
    #: The storage volume for the audio (admins only). None: the server's default.
    volume: str | None = None
    #: Don't credit COMPOSER as producers (e.g. store downloads, where it's the
    #: songwriter's legal name).
    no_composers: bool = False
    #: A file already stored only gets this origin added, instead of what's new in this
    #: import being merged into its track.
    no_duplicate_merge: bool = False
    given: Given | None = None
    #: Put the new track on the Review page with this note ("fix later").
    flag: str | None = None
    #: Ids given for a name that isn't in the tags may go to the one tag artist without
    #: ids (see the README, `--lax-artist-name`).
    lax_artist_names: bool = False
    #: Where the file is on the sending machine (kept as `files.imported_from`).
    imported_from: str | None = None
    #: All of the metadata, instead of the file's tags (see `Metadata`). Then `artists`,
    #: `album`, `first_artist_sources`, `given` and the track's date options aren't used:
    #: everything goes in here.
    metadata: Metadata | None = None

    def check(self) -> None:
        """Raises `InvalidDescription` for anything the server would refuse."""
        self.to_json(lambda image: "image")

    def to_json(self, name_image: Callable[[Image], str]) -> dict[str, Any]:
        """The description as the server takes it. `name_image` gives each image the name
        of the form part it's sent in."""
        out: dict[str, Any] = {"origin": _source(self.origin, TRACK, "origin")}
        if self.metadata is not None:
            refused = [
                ("artists", bool(self.artists), "give each artist's ids on its Credit"),
                ("album", self.album is not None, "the album goes in metadata.album"),
                ("first_artist_sources", bool(self.first_artist_sources), "give the ids on the Credit"),
                ("given", self.given is not None, "metadata has the values"),
                ("lax_artist_names", self.lax_artist_names, "credits are matched by their own ids"),
                ("no_composers", self.no_composers, "producers are only the credits given"),
                ("track.release_date", self.track.release_date is not None, "use metadata.release_date"),
                ("track.release_date_over_tags", self.track.release_date_over_tags, "the tags aren't used"),
                ("track.skip_embedded_covers", self.track.skip_embedded_covers, "embedded pictures aren't used with metadata"),
            ]
            for name, given, instead in refused:
                if given:
                    raise InvalidDescription(f"`{name}` isn't used with `metadata` ({instead})")
            out["metadata"] = _metadata(self.metadata, name_image)
        if self.status is not None:
            _text(self.status, "status")
            out["status"] = self.status

        track = self.track
        track_json: dict[str, Any] = {
            "sources": [_source(s, TRACK, "track.sources") for s in track.sources],
            "covers": [name_image(_image(i, "track.covers")) for i in track.covers],
            "skip_embedded_covers": track.skip_embedded_covers,
            "release_date_over_tags": track.release_date_over_tags,
        }
        if track.release_date is not None:
            track_json["release_date"] = _text(track.release_date, "track.release_date")
        if track.original_filename is not None:
            track_json["original_filename"] = _text(track.original_filename, "track.original_filename")
        out["track"] = track_json

        if self.album is not None:
            album = self.album
            album_json: dict[str, Any] = {
                "sources": [_source(s, ALBUM, "album.sources") for s in album.sources],
                "covers": [name_image(_image(i, "album.covers")) for i in album.covers],
            }
            if album.id is not None:
                album_json["id"] = str(check_uuid(album.id, "album.id"))
            if album.track_number is not None:
                if isinstance(album.track_number, bool) or not isinstance(album.track_number, int) or not 0 <= album.track_number <= 32767:
                    raise InvalidDescription(f"album.track_number: {album.track_number!r} isn't a track number")
                album_json["track_number"] = album.track_number
            if album.single is not None:
                album_json["single"] = bool(album.single)
            if album.release_date is not None:
                album_json["release_date"] = _text(album.release_date, "album.release_date")
            out["album"] = album_json

        artists = []
        for artist in self.artists:
            name = _text(artist.name, "artists[].name")
            if any(a["name"] == name for a in artists):
                raise InvalidDescription(f'artist "{name}" is listed twice')
            artist_json: dict[str, Any] = {
                "name": name,
                "sources": [_source(s, ARTIST, f'artists["{name}"].sources') for s in artist.sources],
            }
            if artist.id is not None:
                artist_json["id"] = str(check_uuid(artist.id, f'artists["{name}"].id'))
            artists.append(artist_json)
        out["artists"] = artists

        if self.first_artist_sources:
            out["first_artist"] = {
                "sources": [_source(s, ARTIST, "first_artist_sources") for s in self.first_artist_sources]
            }
        if self.restrict_to is not None:
            out["restrict_to"] = [str(check_uuid(g, "restrict_to")) for g in self.restrict_to]
        if self.volume is not None:
            out["volume"] = _text(self.volume, "volume")
        out["no_composers"] = self.no_composers
        out["no_duplicate_merge"] = self.no_duplicate_merge
        if self.given is not None:
            given = self.given
            if given.title is not None:
                _text(given.title, "given.title")
            for a in given.artists:
                _text(a, "given.artists[]")
            out["given"] = {
                "title": given.title,
                "title_from": given.title_from,
                "artists": list(given.artists),
                "artists_from": given.artists_from,
                "artists_over_tags": given.artists_over_tags,
            }
        if self.flag is not None and self.flag.strip():
            out["flag"] = self.flag
        out["lax_artist_names"] = self.lax_artist_names
        if self.imported_from is not None:
            out["imported_from"] = _text(self.imported_from, "imported_from")
        return out


def _source(source: Source, kind: str, where: str) -> dict[str, Any]:
    if not isinstance(source, Source):
        raise InvalidDescription(f"{where}: {source!r} isn't a source (use e.g. NetEaseTrack(...) or RawSource(...))")
    if source.kind != kind:
        raise InvalidDescription(
            f"{where}: a {source.platform} {source.kind} source where {'an' if kind == ARTIST else 'a'} {kind} source goes"
        )
    return source.to_json()


def _text(value: Any, what: str) -> str:
    if not isinstance(value, str):
        raise InvalidDescription(f"{what}: {value!r} isn't text")
    return value


def _image(image: Any, what: str) -> Image:
    if isinstance(image, (bytes, bytearray)):
        if not image:
            raise InvalidDescription(f"{what}: an empty image")
        return bytes(image)
    if isinstance(image, (str, os.PathLike)):
        if not os.path.isfile(image):
            raise InvalidDescription(f"{what}: no such file: {os.fspath(image)}")
        return image
    raise InvalidDescription(f"{what}: {image!r} isn't an image (bytes, or a file's path)")


def _metadata(metadata: Metadata, name_image: Callable[[Image], str]) -> dict[str, Any]:
    if not isinstance(metadata.title, str) or not metadata.title.strip():
        raise InvalidDescription("metadata.title is empty")
    # Who each credit is, as the server tells them apart: the same name is the same artist,
    # unless their ids differ (Acetate ids, or ids on the same platform). (name, id, {platform: id})
    artists: list[tuple[str, str | None, dict[str, Any]]] = []

    def who(name: str, id: str | None, sources: list[Source]) -> int:
        ids = {}
        for source in sources:
            shape = SHAPES.get((source.platform, source.kind))
            if shape and shape.id_field and source.data().get(shape.id_field) is not None:
                ids[source.platform] = source.data()[shape.id_field]
        for n, (other, other_id, other_ids) in enumerate(artists):
            differ = (id and other_id and id != other_id) or any(
                other_ids.get(platform) not in (None, value) for platform, value in ids.items()
            )
            if other == name and not differ:
                artists[n] = (other, other_id or id, {**other_ids, **ids})
                return n
        artists.append((name, id, ids))
        return len(artists) - 1

    def credit(c: Credit, where: str) -> dict[str, Any]:
        if not isinstance(c, Credit):
            raise InvalidDescription(f"{where}: {c!r} isn't a Credit")
        name = _text(c.name, f"{where}[].name")
        if not name.strip():
            raise InvalidDescription(f"{where}: a credit without a name")
        if not isinstance(c.role, str) or not c.role.strip():
            raise InvalidDescription(f'{where} "{name}": no role')
        out: dict[str, Any] = {
            "name": name,
            "role": c.role,
            "sources": [_source(s, ARTIST, f'{where}["{name}"].sources') for s in c.sources],
        }
        if c.id is not None:
            out["id"] = str(check_uuid(c.id, f'{where}["{name}"].id'))
        out["_who"] = who(name, out.get("id"), c.sources)
        return out

    credits = []
    for c in metadata.credits:
        c_json = credit(c, "metadata.credits")
        if any(x["_who"] == c_json["_who"] and x["role"] == c_json["role"] for x in credits):
            raise InvalidDescription(f'metadata.credits: "{c_json["name"]}" is credited twice as {c_json["role"]}')
        credits.append(c_json)
    if not any(c["role"] == "primary" for c in credits):
        raise InvalidDescription("metadata.credits: at least one primary artist is needed")
    out: dict[str, Any] = {"title": metadata.title, "credits": credits, "genres": [_text(g, "metadata.genres[]") for g in metadata.genres]}
    if metadata.release_date is not None:
        out["release_date"] = _text(metadata.release_date, "metadata.release_date")
    if metadata.comment is not None:
        out["comment"] = _text(metadata.comment, "metadata.comment")
    if metadata.album is not None:
        album = metadata.album
        if not isinstance(album.title, str) or not album.title.strip():
            raise InvalidDescription("metadata.album.title is empty")
        album_json: dict[str, Any] = {
            "title": album.title,
            "kind": _text(album.kind, "metadata.album.kind"),
            "artists": [credit(c, "metadata.album.artists") for c in album.artists],
            "sources": [_source(s, ALBUM, "metadata.album.sources") for s in album.sources],
            "covers": [name_image(_image(i, "metadata.album.covers")) for i in album.covers],
        }
        for key in ("disc_number", "track_number"):
            value = getattr(album, key)
            if value is not None:
                if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 32767:
                    raise InvalidDescription(f"metadata.album.{key}: {value!r} isn't a number for that")
                album_json[key] = value
        if album.release_date is not None:
            album_json["release_date"] = _text(album.release_date, "metadata.album.release_date")
        if album.id is not None:
            album_json["id"] = str(check_uuid(album.id, "metadata.album.id"))
        out["album"] = album_json
    lyrics = []
    for entry in metadata.lyrics:
        if not isinstance(entry, Lyrics):
            raise InvalidDescription(f"metadata.lyrics: {entry!r} isn't Lyrics")
        item: dict[str, Any] = {"body": _text(entry.body, "metadata.lyrics[].body"), "official": bool(entry.official)}
        if entry.language is not None:
            item["language"] = _text(entry.language, "metadata.lyrics[].language")
        lyrics.append(item)
    out["lyrics"] = lyrics
    for c in [*out["credits"], *out.get("album", {}).get("artists", [])]:
        del c["_who"]
    return out
