"""Sending music to an Acetate server, with what you know about it.

    from acetate_ingest import Acetate, Description, ArtistHint, AlbumHint, NetEaseTrack, NetEaseArtist, NetEaseAlbum

    acetate = Acetate.from_env()
    report = acetate.ingest(
        "Ravel. - Song.flac",
        Description(
            origin=NetEaseTrack(song_id=1000000111),
            artists=[ArtistHint("Ravel.", sources=[NetEaseArtist(artist_id=10000037)])],
            album=AlbumHint(sources=[NetEaseAlbum(album_id=100000074)], covers=["cover.jpg"]),
        ),
    )
    print(report.summary())

See the project's README.
"""

__version__ = "0.2.0"

from ._shapes import *  # noqa: E402,F403  (the typed sources: NetEaseTrack, SoundCloudArtist, ...)
from ._shapes import SHAPES, __all__ as _shape_names  # noqa: E402
from ._source import ALBUM, ARTIST, TRACK, Field, RawSource, Shape, Source  # noqa: E402
from ._extensions import load_extension_files  # noqa: E402
from .client import Acetate  # noqa: E402
from .description import (  # noqa: E402
    Album,
    AlbumHint,
    ArtistHint,
    Credit,
    Description,
    Given,
    Image,
    Lyrics,
    Metadata,
    TrackHint,
)
from .errors import (  # noqa: E402
    AcetateError,
    CantConnect,
    InvalidDescription,
    NotLoggedIn,
    RateLimited,
    ServerError,
    TooLarge,
    UploadQuotaExceeded,
)
from .report import Report, Resolved  # noqa: E402

__all__ = [
    "Acetate",
    "Description",
    "TrackHint",
    "AlbumHint",
    "ArtistHint",
    "Given",
    "Image",
    "Metadata",
    "Credit",
    "Album",
    "Lyrics",
    "Source",
    "RawSource",
    "load_extension_files",
    "SHAPES",
    "Shape",
    "Field",
    "TRACK",
    "ALBUM",
    "ARTIST",
    "Report",
    "Resolved",
    "AcetateError",
    "InvalidDescription",
    "CantConnect",
    "ServerError",
    "NotLoggedIn",
    "TooLarge",
    "UploadQuotaExceeded",
    "RateLimited",
    *_shape_names,
]
