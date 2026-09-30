"""Against a real Acetate server: skipped unless ACETATE_TEST_URL, ACETATE_TEST_USER and
ACETATE_TEST_PASSWORD are set (an admin). It imports the repository's test-audio files,
so use a test server, never a real library:

    ACETATE_BIND=127.0.0.1:3001 ACETATE_DATA_DIR=/tmp/acetate-test DATABASE_URL=.../acetate_scratch acetate serve
    ACETATE_TEST_URL=http://127.0.0.1:3001 ACETATE_TEST_USER=... ACETATE_TEST_PASSWORD=... python -m unittest
"""

import os
import struct
import unittest
import uuid
import zlib

from acetate_ingest import (
    Acetate,
    Album,
    AlbumHint,
    ArtistHint,
    Credit,
    CustomTrack,
    Description,
    Given,
    Lyrics,
    Metadata,
    NetEaseAlbum,
    NetEaseArtist,
    NetEaseTrack,
    ServerError,
    TrackHint,
)

AUDIO = os.path.join(os.path.dirname(__file__), "..", "..", "..", "test-audio")
URL = os.environ.get("ACETATE_TEST_URL")


def png(red: int) -> bytes:
    """A 1×1 PNG of its own colour (so each is a different image)."""

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    header = struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
    pixels = zlib.compress(bytes([0, red, 0, 0]))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", pixels) + chunk(b"IEND", b"")


@unittest.skipUnless(URL, "ACETATE_TEST_URL isn't set")
class AgainstAServer(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.acetate = Acetate.login(URL, os.environ["ACETATE_TEST_USER"], os.environ["ACETATE_TEST_PASSWORD"])

    def test_the_shapes_are_the_servers(self):
        self.assertEqual(self.acetate.shape_differences(), [])

    def test_a_dry_run_then_for_real_then_again(self):
        path = os.path.join(AUDIO, "01 - Glass Harbor.flac")
        song_id = uuid.uuid4().int % 10**12  # new each run
        description = Description(
            origin=NetEaseTrack(song_id=song_id, song_index=1),
            artists=[ArtistHint("Mira Vale", sources=[NetEaseArtist(artist_id=song_id)])],
            album=AlbumHint(sources=[NetEaseAlbum(album_id=song_id)], covers=[png(song_id % 256)]),
            track=TrackHint(release_date="2019-05-12"),
            flag="from the Python library's tests",
        )

        dry = self.acetate.ingest(path, description, dry_run=True)
        self.assertTrue(dry.dry_run)
        self.assertIn(dry.status, ("created", "duplicate"))

        first = self.acetate.ingest(path, description)
        self.assertFalse(first.dry_run)
        if first.created:  # the first run on this server
            self.assertTrue(first.track_id)
            self.assertEqual(first.album_covers_added, 1, first.summary())
        # The same bytes again: stored already.
        again = self.acetate.ingest(path, description)
        self.assertTrue(again.duplicate, again.summary())

    def test_bytes_and_a_custom_origin(self):
        with open(os.path.join(AUDIO, "lantern demo.mp3"), "rb") as f:
            audio = f.read()
        report = self.acetate.ingest(
            audio,
            Description(origin=CustomTrack(label="discord", description="the Python library's tests"), status="demo"),
            filename="lantern demo.mp3",
            dry_run=True,
        )
        self.assertIn(report.status, ("created", "duplicate"))

    def test_values_given_for_a_file_without_tags(self):
        path = os.path.join(AUDIO, "no-artist.opus")
        description = Description(
            origin=CustomTrack(label="tests"),
            given=Given(artists=["Someone"], artists_from="the Python library's tests"),
        )
        report = self.acetate.ingest(path, description, dry_run=True)
        self.assertIn(report.status, ("created", "duplicate"))

    def test_all_given_the_tags_unused(self):
        # A file with tags (Mira Vale, "Glass Harbor"): with `metadata`, none of them is used.
        song_id = uuid.uuid4().int % 10**12
        description = Description(
            origin=NetEaseTrack(song_id=song_id),
            metadata=Metadata(
                title="Given Title",
                credits=[Credit("Given Artist", sources=[NetEaseArtist(artist_id=song_id)]), Credit("Given Producer", "producer")],
                album=Album(title="Given EP", kind="ep", track_number=2, covers=[png(song_id % 256)]),
                lyrics=[Lyrics("words", language="en")],
            ),
        )
        report = self.acetate.ingest(os.path.join(AUDIO, "01 - Glass Harbor.flac"), description, dry_run=True)
        self.assertEqual({a.name for a in report.artists}, {"Given Artist", "Given Producer"}, report.summary())
        self.assertEqual(report.album.name, "Given EP")
        # No artist in its tags at all: fine.
        report = self.acetate.ingest(os.path.join(AUDIO, "no-artist.opus"), description, dry_run=True)
        self.assertIn(report.status, ("created", "duplicate"))
        # A role the server doesn't have: its message says which there are.
        description.metadata.credits.append(Credit("Someone", "vibes"))
        with self.assertRaises(ServerError) as caught:
            self.acetate.ingest(os.path.join(AUDIO, "01 - Glass Harbor.flac"), description, dry_run=True)
        self.assertIn("vibes", caught.exception.message)

    def test_the_server_says_what_it_refuses(self):
        path = os.path.join(AUDIO, "lantern demo.mp3")
        with self.assertRaises(ServerError) as caught:
            self.acetate.ingest(
                path,
                Description(origin=CustomTrack(label="tests"), restrict_to=[str(uuid.uuid4())]),
                dry_run=True,
            )
        self.assertTrue(caught.exception.message)


if __name__ == "__main__":
    unittest.main()
