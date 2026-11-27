"""Against a real Acetate server: skipped unless ACETATE_TEST_URL, ACETATE_TEST_USER and
ACETATE_TEST_PASSWORD are set (an admin). It imports the repository's test-audio files,
so use a test server, never a real library:

    ACETATE_BIND=127.0.0.1:3001 ACETATE_DATA_DIR=/tmp/acetate-test DATABASE_URL=.../acetate_scratch acetate serve
    ACETATE_TEST_URL=http://127.0.0.1:3001 ACETATE_TEST_USER=... ACETATE_TEST_PASSWORD=... python -m unittest
"""

import hashlib
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
from acetate_ingest import AlreadyKnown

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

    def test_a_big_file_goes_in_chunks(self):
        # Small pieces, so the test file counts as big.
        chunked = Acetate(self.acetate.url, self.acetate.token, chunk_size=20_000)
        report = chunked.ingest(
            os.path.join(AUDIO, "lantern demo.mp3"),
            Description(origin=CustomTrack(label="discord", description="the Python library's tests")),
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


    def test_lookups_then_a_proposal_adopted_and_one_rejected(self):
        song_id = uuid.uuid4().int % 10**12
        description = Description(origin=NetEaseTrack(song_id=song_id), flag="from the Python library's tests")
        path = os.path.join(AUDIO, "lantern demo.mp3")
        report = self.acetate.ingest(path, description)
        with open(path, "rb") as f:
            sha256 = hashlib.sha256(f.read()).hexdigest()
        track_id = report.track_id or self.acetate.lookup_files([sha256])[0]["tracks"][0]["track_id"]

        [hit] = self.acetate.lookup_files([sha256])
        self.assertIn(track_id, [t["track_id"] for t in hit["tracks"]])
        self.assertEqual(self.acetate.lookup_files(["0" * 64])[0]["tracks"], [])
        if report.created:
            [owners] = self.acetate.lookup_sources([("netease", str(song_id))])
            self.assertEqual(owners["owners"], [{"kind": "track", "id": track_id}])
        title = self.acetate.track(track_id)["title"]
        self.assertIn(track_id, [t["id"] for t in self.acetate.search_tracks(title)])

        variant = next(v for v in self.acetate.track(track_id)["variants"])
        key = f"date:{song_id}"
        proposal = {
            "key": key,
            "agent": "python tests",
            "base_revision_id": variant["revision_id"],
            "patch": {"release_date": "2001-02-03"},
            "evidence": [{"label": "a page", "url": "https://example.com", "quote": "Feb 3, 2001"}],
        }
        self.assertIsNone(self.acetate.propose(track_id, proposal)["tracks"][0].get("error"))
        self.acetate.propose(track_id, proposal)  # the same again: nothing more
        open_flags = [f for f in self.acetate.review_flags(kind="proposal", track_id=track_id) if f["related"]["key"] == key]
        self.assertEqual(len(open_flags), 1)
        self.assertEqual(open_flags[0]["related"]["fields"], ["release_date"])

        answer = self.acetate._request("POST", f"/api/review/{open_flags[0]['id']}/adopt", json_body={})
        self.assertIsNone(answer["tracks"][0].get("error"))
        self.assertEqual(self.acetate.track(track_id)["release_date"], "2001-02-03")

        # Against the revision before the date: refused now.
        stale = {**proposal, "key": f"title:{song_id}", "patch": {"title": "Changed"}}
        self.acetate.propose(track_id, stale)
        [flag] = [f for f in self.acetate.review_flags(kind="proposal", track_id=track_id) if f["related"]["key"] == stale["key"]]
        with self.assertRaises(ServerError) as refused:
            self.acetate._request("POST", f"/api/review/{flag['id']}/adopt", json_body={})
        self.assertEqual(refused.exception.status, 409)
        self.acetate._request("POST", f"/api/review/{flag['id']}/resolve", json_body={"resolution": "rejected: tests"})
        settled = [f for f in self.acetate.review_flags(kind="proposal", resolved=True, track_id=track_id) if f["id"] == flag["id"]]
        self.assertEqual(settled[0]["resolution"], "rejected: tests")

    def test_the_catalog_then_proposals_adopted_in_bulk(self):
        song_id = uuid.uuid4().int % 10**12
        description = Description(origin=NetEaseTrack(song_id=song_id), flag="from the Python library's tests")
        path = os.path.join(AUDIO, "lantern demo.mp3")
        report = self.acetate.ingest(path, description)
        with open(path, "rb") as f:
            sha256 = hashlib.sha256(f.read()).hexdigest()
        track_id = report.track_id or self.acetate.lookup_files([sha256])[0]["tracks"][0]["track_id"]

        catalog = self.acetate.catalog()
        self.assertTrue(catalog["version"])
        [row] = [t for t in catalog["tracks"] if t["id"] == track_id]
        self.assertIn("platforms", row)

        variant = next(v for v in self.acetate.track(track_id)["variants"])
        day = f"19{song_id % 90 + 10}-01-02"
        date = {"key": f"bulk-date:{song_id}", "agent": "python tests", "job_id": "bulk", "base_revision_id": variant["revision_id"], "patch": {"release_date": day}}
        title = {**date, "key": f"bulk-title:{song_id}", "patch": {"title": f"Bulk {song_id}"}}
        for proposal in (date, title):
            self.acetate.propose(track_id, proposal)
        flags = {f["related"]["key"]: f["id"] for f in self.acetate.review_flags(kind="proposal", track_id=track_id)}
        ids = [flags[date["key"]], flags[title["key"]], str(uuid.uuid4())]
        before = self.acetate.track(track_id)["release_date"]

        preview = self.acetate.adopt_many(ids, preview=True)
        self.assertEqual((preview["adopted"], preview["stale"], preview["missing"]), (ids[:1], ids[1:2], ids[2:]))
        self.assertEqual(self.acetate.track(track_id)["release_date"], before, "a preview keeps nothing")
        self.assertEqual(self.acetate.adopt_many(ids), preview)
        self.assertEqual(self.acetate.track(track_id)["release_date"], day)
        open_keys = {f["related"]["key"] for f in self.acetate.review_flags(kind="proposal", track_id=track_id)}
        self.assertNotIn(date["key"], open_keys)
        self.assertIn(title["key"], open_keys, "the stale one is left open")
        self.acetate._request("POST", f"/api/review/{ids[1]}/resolve", json_body={"resolution": "rejected: tests"})

    def test_a_lost_song_made_refused_found(self):
        song_id = uuid.uuid4().int % 10**12
        description = Description(origin=NetEaseTrack(song_id=song_id), flag="from the Python library's tests")
        path = os.path.join(AUDIO, "lantern demo.mp3")
        report = self.acetate.ingest(path, description)
        with open(path, "rb") as f:
            sha256 = hashlib.sha256(f.read()).hexdigest()
        track_id = report.track_id or self.acetate.lookup_files([sha256])[0]["tracks"][0]["track_id"]
        # (Imported by another test already: it gets this run's id.)
        held = {"platform": "netease", "data": {"song_id": song_id}}
        self.acetate.edit_tracks({"tracks": [{"track_id": track_id, "sources": {"add": [held]}}], "reason": "tests"})
        artist_id = self.acetate.track(track_id)["credits"][0]["artist_id"]
        lost = {"status": "proposed", "artists_shown": "Mira Vale ft. Someone", "release_date": "2019-05"}
        gone = {"platform": "netease", "data": {"song_id": song_id + 1}}

        preview = self.acetate.create_lost_song("Gone", [artist_id], lost, sources=[gone], preview=True)
        self.assertTrue(preview["preview"])
        self.assertEqual(self.acetate.lookup_sources([("netease", str(song_id + 1))])[0]["owners"], [], "nothing kept")

        made = self.acetate.create_lost_song("Gone", [artist_id], lost, sources=[gone], reason="tests")
        [hit] = self.acetate.lookup_sources([("netease", str(song_id + 1))])
        self.assertEqual(hit["owners"], [{"kind": "lost_song", "id": made["id"]}])
        listed = self.acetate.lost_songs(artist_id=artist_id, status="proposed")
        self.assertIn(made["id"], [e["id"] for e in listed])
        with self.assertRaises(AlreadyKnown) as again:
            self.acetate.create_lost_song("Gone again", [artist_id], lost, sources=[gone])
        self.assertEqual((again.exception.code, again.exception.id), ("already_lost", made["id"]))
        with self.assertRaises(AlreadyKnown) as refused:
            self.acetate.create_lost_song("Not gone", [artist_id], lost, sources=[held])
        self.assertEqual((refused.exception.code, refused.exception.id), ("already_held", track_id))

        added = self.acetate.add_extra_files(made["id"], [(f"cover-{song_id}.png", png(song_id % 251))])
        self.assertEqual(len(added["files"]), 1, added)

        # The track gets the lost song's platform id: flagged, then found.
        edit = {"tracks": [{"track_id": track_id, "sources": {"add": [gone]}}], "reason": "tests"}
        self.assertIsNone(self.acetate.edit_tracks(edit)["tracks"][0].get("error"))
        [flag] = self.acetate.review_flags(kind="lost_song_found", track_id=track_id)
        self.assertEqual(flag["related"]["extra_id"], made["id"])
        found = self.acetate._request("POST", f"/api/review/{flag['id']}/found", json_body={})
        self.assertEqual((found["track_id"], found["extra_id"]), (track_id, made["id"]))
        self.assertEqual(self.acetate.lookup_sources([("netease", str(song_id + 1))])[0]["owners"], [{"kind": "track", "id": track_id}])


if __name__ == "__main__":
    unittest.main()
