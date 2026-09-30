"""What ingesting one file did (or, in a dry run, would do)."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True)
class Resolved:
    """An artist or album the tags named, and how it was found."""

    #: The name in the tags (an album: its title).
    name: str
    #: Its Acetate id. None: it's one you can't see (a track restricted away from you).
    id: str | None
    #: "id" (the description gave it), "platform_id" (it had one of the ids given),
    #: "name" (by name; an album by title, album artists and date) or "created".
    matched_by: str

    @classmethod
    def from_json(cls, data: dict[str, Any]) -> Resolved:
        return cls(name=data["name"], id=data.get("id"), matched_by=data["matched_by"])


@dataclass(frozen=True)
class Report:
    """The server's answer to one file."""

    #: "created" (a new track) or "duplicate" (these exact bytes were stored already: this
    #: import's new parts were added to that track, see `added`).
    status: str
    #: None for a duplicate of a track you can't see.
    track_id: str | None
    #: True: nothing was kept ("created" means "would be created").
    dry_run: bool
    #: Tracks that already had one of this file's platform song ids: probably the same song
    #: in another file. The new track is flagged for review with them.
    same_song_as: list[str] = field(default_factory=list)
    artists: list[Resolved] = field(default_factory=list)
    album: Resolved | None = None
    album_covers_added: int = 0
    warnings: list[str] = field(default_factory=list)
    #: For a duplicate: what this import added to its track, e.g. 'on the single "X"'.
    added: list[str] = field(default_factory=list)
    #: Names in the tags that are collectives (groupings): the track went into them, and
    #: no one was credited as them.
    collectives: list[Resolved] = field(default_factory=list)
    #: The answer as the server sent it.
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    @property
    def created(self) -> bool:
        return self.status == "created"

    @property
    def duplicate(self) -> bool:
        return self.status == "duplicate"

    @classmethod
    def from_json(cls, data: dict[str, Any]) -> Report:
        album = data.get("album")
        return cls(
            status=data["status"],
            track_id=data.get("track_id"),
            dry_run=bool(data.get("dry_run", False)),
            same_song_as=list(data.get("same_song_as") or []),
            artists=[Resolved.from_json(a) for a in data.get("artists") or []],
            album=Resolved.from_json(album) if album else None,
            album_covers_added=int(data.get("album_covers_added") or 0),
            warnings=list(data.get("warnings") or []),
            added=list(data.get("added") or []),
            collectives=[Resolved.from_json(c) for c in data.get("collectives") or []],
            raw=data,
        )

    def summary(self) -> str:
        """One line for a log: what happened, and the warnings."""
        verb = {"created": "would create" if self.dry_run else "created", "duplicate": "already stored"}.get(
            self.status, self.status
        )
        parts = [f"{verb} {self.track_id or '(a track you can not see)'}"]
        if self.album:
            parts.append(f'album "{self.album.name}" ({self.album.matched_by})')
        if self.artists:
            parts.append("artists " + ", ".join(f"{a.name} ({a.matched_by})" for a in self.artists))
        parts += [f"+ {a}" for a in self.added]
        parts += [f"warning: {w}" for w in self.warnings]
        return "; ".join(parts)
