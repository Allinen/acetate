"""Sources: where a track, album or artist can be found, as a platform and its data.

Each platform has a strict shape per kind of thing (its fields, their types, which are
required), the same one the server checks. The typed classes (`NetEaseTrack`,
`SoundCloudArtist`, ...) are made from the server's shapes, see `_shapes.py`. `RawSource`
takes a platform, a kind and a dict instead, checked the same way.
"""

from __future__ import annotations

import dataclasses
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any, NamedTuple

from .errors import InvalidDescription

TRACK = "track"
ALBUM = "album"
ARTIST = "artist"


class Field(NamedTuple):
    """A field of a shape: its type ("int", "str" or "strs", a list of strings) and
    whether it must be given."""

    type: str
    required: bool


class Shape(NamedTuple):
    """A platform's data for one kind of thing. `id_field` holds the platform's id for it
    (data with the same id is the same source); None if the platform has no id."""

    id_field: str | None
    fields: dict[str, Field]


def check_data(platform: str, kind: str, data: dict[str, Any]) -> None:
    """Raises `InvalidDescription` unless `data` fits the platform's shape for `kind`,
    as the server would check it."""
    from ._shapes import SHAPES

    def fail(message: str) -> None:
        raise InvalidDescription(f"{platform} {kind}: {message}")

    shape = SHAPES.get((platform, kind))
    if shape is None:
        known = sorted(p for p, k in SHAPES if k == kind)
        fail(f"no data shape defined (platforms with {kind} sources: {', '.join(known)})")
    for key in data:
        if key not in shape.fields:
            fail(f"unknown field `{key}`, expected one of {list(shape.fields)}")
    for name, field in shape.fields.items():
        value = data.get(name)
        if value is None:
            if field.required:
                fail(f"`{name}` is required")
            continue
        if field.type == "int":
            ok = isinstance(value, int) and not isinstance(value, bool)
        elif field.type == "str":
            ok = isinstance(value, str)
        else:
            ok = isinstance(value, (list, tuple)) and all(isinstance(item, str) for item in value)
        if not ok:
            expected = {"int": "an integer", "str": "a string", "strs": "a list of strings"}[field.type]
            fail(f"`{name}` must be {expected}, got {value!r}")


class Source:
    """One presence of a track, album or artist on one platform, as it is now.

    `source_id` adds the data to an existing source (only needed for platforms without an
    id, e.g. a custom source whose description changed); `captured_at` is when the data was
    seen on the platform (default: when the server gets it).
    """

    platform: str
    kind: str
    source_id: str | uuid.UUID | None
    captured_at: datetime | None

    def data(self) -> dict[str, Any]:
        raise NotImplementedError

    def to_json(self) -> dict[str, Any]:
        """The source as the server takes it."""
        out: dict[str, Any] = {"platform": self.platform, "data": self.data()}
        if self.source_id is not None:
            out["source_id"] = str(check_uuid(self.source_id, "source_id"))
        if self.captured_at is not None:
            captured = self.captured_at
            if captured.tzinfo is None:
                raise InvalidDescription(f"{self.platform} {self.kind}: `captured_at` needs a time zone")
            out["captured_at"] = captured.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")
        return out


@dataclass(frozen=True, kw_only=True)
class ShapedSource(Source):
    """The base of the typed sources: their fields are the shape's."""

    source_id: str | uuid.UUID | None = None
    captured_at: datetime | None = None

    def data(self) -> dict[str, Any]:
        out = {}
        for field in dataclasses.fields(self):
            if field.name in ("source_id", "captured_at"):
                continue
            value = getattr(self, field.name)
            if value is not None:
                out[field.name] = list(value) if isinstance(value, tuple) else value
        return out

    def __post_init__(self) -> None:
        check_data(self.platform, self.kind, self.data())


class RawSource(Source):
    """A source given as a platform, a kind ("track", "album" or "artist") and its data,
    for code that has them as values. Checked like the typed ones."""

    def __init__(
        self,
        platform: str,
        kind: str,
        data: dict[str, Any],
        *,
        source_id: str | uuid.UUID | None = None,
        captured_at: datetime | None = None,
    ) -> None:
        if kind not in (TRACK, ALBUM, ARTIST):
            raise InvalidDescription(f"a source's kind is track, album or artist, not {kind!r}")
        self.platform = platform
        self.kind = kind
        self._data = {k: list(v) if isinstance(v, tuple) else v for k, v in data.items() if v is not None}
        self.source_id = source_id
        self.captured_at = captured_at
        check_data(platform, kind, self._data)

    def data(self) -> dict[str, Any]:
        return dict(self._data)

    def __repr__(self) -> str:
        return f"RawSource({self.platform!r}, {self.kind!r}, {self._data!r})"


def check_uuid(value: str | uuid.UUID, what: str) -> uuid.UUID:
    """`value` as a UUID, or `InvalidDescription` saying what it was for."""
    if isinstance(value, uuid.UUID):
        return value
    try:
        return uuid.UUID(str(value))
    except ValueError:
        raise InvalidDescription(f"{what}: {value!r} isn't an id (a UUID)") from None
