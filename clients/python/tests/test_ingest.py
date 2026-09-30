"""Tests without a server (a stand-in answers). `python -m unittest` in clients/python.

A real server: see test_server.py.
"""

import email.parser
import email.policy
import http.server
import json
import os
import tempfile
import threading
import unittest
from datetime import datetime, timezone

from acetate_ingest import (
    Acetate,
    Album,
    AlbumHint,
    ArtistHint,
    Credit,
    CustomTrack,
    Description,
    Given,
    InvalidDescription,
    Lyrics,
    Metadata,
    NetEaseAlbum,
    NetEaseArtist,
    NetEaseTrack,
    NotLoggedIn,
    RawSource,
    ServerError,
    SoundCloudTrack,
    TrackHint,
    UploadQuotaExceeded,
)


class Sources(unittest.TestCase):
    def test_typed_sources_check_their_fields(self):
        self.assertEqual(NetEaseTrack(song_id=5).to_json(), {"platform": "netease", "data": {"song_id": 5}})
        with self.assertRaises(TypeError):
            NetEaseTrack()  # a required field: a TypeError, as for any missing argument
        with self.assertRaisesRegex(InvalidDescription, "`song_id` is required"):
            NetEaseTrack(song_id=None)
        with self.assertRaisesRegex(InvalidDescription, "must be an integer"):
            NetEaseTrack(song_id="5")
        with self.assertRaisesRegex(InvalidDescription, "must be an integer"):
            NetEaseTrack(song_id=True)
        with self.assertRaisesRegex(InvalidDescription, "a list of strings"):
            SoundCloudTrack(song_permalinks="a/b")

    def test_lists_and_optional_fields(self):
        source = SoundCloudTrack(song_id=1, song_permalinks=("a/b", "a/c"))
        self.assertEqual(source.data(), {"song_id": 1, "song_permalinks": ["a/b", "a/c"]})

    def test_captured_at_and_source_id(self):
        at = datetime(2023, 5, 1, 12, 0, tzinfo=timezone.utc)
        source = CustomTrack(label="discord", captured_at=at, source_id="0192f000-0000-7000-8000-000000000000")
        self.assertEqual(source.to_json()["captured_at"], "2023-05-01T12:00:00Z")
        with self.assertRaisesRegex(InvalidDescription, "time zone"):
            CustomTrack(label="x", captured_at=datetime(2023, 5, 1)).to_json()
        with self.assertRaisesRegex(InvalidDescription, "isn't an id"):
            CustomTrack(label="x", source_id="nope").to_json()

    def test_raw_sources_are_checked_the_same(self):
        self.assertEqual(RawSource("netease", "artist", {"artist_id": 3}).data(), {"artist_id": 3})
        with self.assertRaisesRegex(InvalidDescription, "unknown field `song_idd`"):
            RawSource("netease", "track", {"song_idd": 3})
        with self.assertRaisesRegex(InvalidDescription, "no data shape"):
            RawSource("myspace", "track", {})
        with self.assertRaisesRegex(InvalidDescription, "no data shape"):
            RawSource("youtube", "album", {})


class Descriptions(unittest.TestCase):
    def test_what_goes_where(self):
        description = Description(
            origin=NetEaseTrack(song_id=1),
            artists=[ArtistHint("Ravel.", sources=[NetEaseArtist(artist_id=2)])],
            album=AlbumHint(sources=[NetEaseAlbum(album_id=3)], covers=[b"front", b"back"], track_number=4),
            track=TrackHint(covers=[b"track"], original_filename="song final.wav"),
            given=Given(title="Song", title_from="the file name"),
            flag="  ",
        )
        names = iter(["a", "b", "c"])
        body = description.to_json(lambda image: next(names))
        self.assertEqual(body["origin"], {"platform": "netease", "data": {"song_id": 1}})
        self.assertEqual(body["artists"], [{"name": "Ravel.", "sources": [{"platform": "netease", "data": {"artist_id": 2}}]}])
        self.assertEqual(body["track"]["covers"], ["a"])
        self.assertEqual(body["album"]["covers"], ["b", "c"])
        self.assertEqual(body["album"]["track_number"], 4)
        self.assertEqual(body["track"]["original_filename"], "song final.wav")
        self.assertEqual(body["given"]["title_from"], "the file name")
        self.assertNotIn("flag", body, "an empty note is none")

    def test_the_wrong_kind_of_source(self):
        with self.assertRaisesRegex(InvalidDescription, "a netease artist source where a track source goes"):
            Description(origin=NetEaseArtist(artist_id=1)).check()
        with self.assertRaisesRegex(InvalidDescription, "album.sources"):
            Description(origin=NetEaseTrack(song_id=1), album=AlbumHint(sources=[NetEaseTrack(song_id=1)])).check()
        with self.assertRaisesRegex(InvalidDescription, "isn't a source"):
            Description(origin={"platform": "netease", "data": {"song_id": 1}}).check()

    def test_other_mistakes(self):
        origin = NetEaseTrack(song_id=1)
        with self.assertRaisesRegex(InvalidDescription, "listed twice"):
            Description(origin=origin, artists=[ArtistHint("A"), ArtistHint("A")]).check()
        with self.assertRaisesRegex(InvalidDescription, "restrict_to"):
            Description(origin=origin, restrict_to=["friends"]).check()
        with self.assertRaisesRegex(InvalidDescription, "track number"):
            Description(origin=origin, album=AlbumHint(track_number=-1)).check()
        with self.assertRaisesRegex(InvalidDescription, "no such file"):
            Description(origin=origin, track=TrackHint(covers=["/nowhere/cover.jpg"])).check()


class AllGiven(unittest.TestCase):
    """With `metadata`: everything from the description, nothing from the tags."""

    def metadata(self, **changes):
        fields = dict(
            title="Real Title",
            credits=[
                Credit("Real Artist", sources=[NetEaseArtist(artist_id=77)]),
                Credit("Guest", "featured"),
                Credit("Real Artist", "producer"),
            ],
            release_date="2020-01-01",
            album=Album(title="Real EP", kind="ep", track_number=3, sources=[NetEaseAlbum(album_id=88)], covers=[b"front"]),
            lyrics=[Lyrics("[00:01.00] line", official=True, language="en")],
        )
        fields.update(changes)
        return Metadata(**fields)

    def test_what_goes_where(self):
        body = Description(origin=NetEaseTrack(song_id=1), metadata=self.metadata()).to_json(lambda image: "front")
        metadata = body["metadata"]
        self.assertEqual([(c["name"], c["role"]) for c in metadata["credits"]],
                         [("Real Artist", "primary"), ("Guest", "featured"), ("Real Artist", "producer")])
        self.assertEqual(metadata["credits"][0]["sources"], [{"platform": "netease", "data": {"artist_id": 77}}])
        self.assertEqual(metadata["album"]["covers"], ["front"])
        self.assertEqual(metadata["album"]["kind"], "ep")
        self.assertEqual(metadata["lyrics"], [{"body": "[00:01.00] line", "official": True, "language": "en"}])

    def test_mistakes(self):
        origin = NetEaseTrack(song_id=1)
        with self.assertRaisesRegex(InvalidDescription, "at least one primary"):
            Description(origin=origin, metadata=self.metadata(credits=[Credit("A", "featured")])).check()
        with self.assertRaisesRegex(InvalidDescription, "credited twice as primary"):
            Description(origin=origin, metadata=self.metadata(credits=[Credit("A"), Credit("A")])).check()
        # Two artists with one name, told apart by their ids: fine, even in the same role.
        ids = ["0192f000-0000-7000-8000-000000000001", "0192f000-0000-7000-8000-000000000002"]
        Description(origin=origin, metadata=self.metadata(credits=[Credit("A", id=ids[0]), Credit("A", id=ids[1])])).check()
        Description(origin=origin, metadata=self.metadata(credits=[
            Credit("A", sources=[NetEaseArtist(artist_id=1)]), Credit("A", sources=[NetEaseArtist(artist_id=2)]),
        ])).check()
        with self.assertRaisesRegex(InvalidDescription, "credited twice"):
            Description(origin=origin, metadata=self.metadata(credits=[
                Credit("A", sources=[NetEaseArtist(artist_id=1)]), Credit("A"),
            ])).check()
        with self.assertRaisesRegex(InvalidDescription, "`artists` isn't used with `metadata`"):
            Description(origin=origin, metadata=self.metadata(), artists=[ArtistHint("A")]).check()
        with self.assertRaisesRegex(InvalidDescription, "`track.release_date` isn't used"):
            Description(origin=origin, metadata=self.metadata(), track=TrackHint(release_date="2020")).check()
        with self.assertRaisesRegex(InvalidDescription, "metadata.title is empty"):
            Description(origin=origin, metadata=self.metadata(title=" ")).check()
        with self.assertRaisesRegex(InvalidDescription, "track source where an artist source goes"):
            Description(origin=origin, metadata=self.metadata(credits=[Credit("A", sources=[NetEaseTrack(song_id=1)])])).check()


class StandIn(http.server.BaseHTTPRequestHandler):
    """Answers like Acetate would; keeps what it got."""

    answer = (200, {}, {})
    got = []

    def do_POST(self):
        length = int(self.headers["Content-Length"])
        StandIn.got.append((self.path, dict(self.headers), self.rfile.read(length)))
        status, headers, body = StandIn.answer
        data = json.dumps(body).encode()
        self.send_response(status)
        for key, value in headers.items():
            self.send_header(key, value)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


class Sending(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), StandIn)
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.url = f"http://127.0.0.1:{cls.server.server_port}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def setUp(self):
        StandIn.got.clear()

    def test_a_file_its_description_and_covers(self):
        StandIn.answer = (200, {}, {
            "status": "created", "track_id": "t1", "dry_run": True, "same_song_as": [],
            "artists": [{"name": "Ravel.", "id": "a1", "matched_by": "platform_id"}],
            "album": {"name": "Album", "id": "b1", "matched_by": "created"},
            "album_covers_added": 1, "warnings": ["w"], "added": [], "collectives": [],
        })
        with tempfile.TemporaryDirectory() as folder:
            audio = os.path.join(folder, "Ravel. - Song.flac")
            with open(audio, "wb") as f:
                f.write(b"fLaC" + os.urandom(3 * 1024 * 1024))  # more than one chunk
            cover = os.path.join(folder, "cover.jpg")
            with open(cover, "wb") as f:
                f.write(b"\xff\xd8jpeg")
            report = Acetate(self.url, "tok").ingest(
                audio,
                Description(origin=NetEaseTrack(song_id=1), album=AlbumHint(covers=[cover])),
                tags_file=b"tagged",
                dry_run=True,
            )
            with open(audio, "rb") as f:
                sent_audio = f.read()

        self.assertTrue(report.created and report.dry_run)
        self.assertEqual(report.album.matched_by, "created")
        self.assertIn("would create t1", report.summary())
        path, headers, body = StandIn.got[0]
        self.assertEqual(path, "/api/ingest?dry_run=true")
        self.assertEqual(headers["Authorization"], "Bearer tok")
        # The body as a multipart parser reads it.
        message = email.parser.BytesParser(policy=email.policy.HTTP).parsebytes(
            b"Content-Type: " + headers["Content-Type"].encode() + b"\r\n\r\n" + body
        )
        parts = {p.get_param("name", header="content-disposition"): p for p in message.iter_parts()}
        self.assertEqual(list(parts), ["description", "file", "tags_file", "image-0"])
        description = json.loads(parts["description"].get_content())
        self.assertEqual(description["album"]["covers"], ["image-0"])
        self.assertEqual(parts["file"].get_filename(), "Ravel. - Song.flac")
        self.assertEqual(parts["file"].get_content(), sent_audio)
        self.assertEqual(parts["tags_file"].get_content(), b"tagged")
        self.assertEqual(parts["image-0"].get_content(), b"\xff\xd8jpeg")

    def test_bytes_need_a_name(self):
        acetate = Acetate(self.url, "tok")
        with self.assertRaisesRegex(InvalidDescription, "filename="):
            acetate.ingest(b"audio", Description(origin=NetEaseTrack(song_id=1)))
        self.assertEqual(StandIn.got, [], "nothing sent")

    def test_errors_carry_the_servers_message(self):
        acetate = Acetate(self.url, "tok")
        description = Description(origin=NetEaseTrack(song_id=1))
        StandIn.answer = (429, {"Retry-After": "3600"}, {"code": "upload_quota", "message": "Today's uploads are used up."})
        with self.assertRaises(UploadQuotaExceeded) as caught:
            acetate.ingest(b"x", description, filename="a.flac")
        self.assertEqual((caught.exception.retry_after, str(caught.exception)), (3600, "Today's uploads are used up."))
        StandIn.answer = (401, {}, {"code": "unauthorized", "message": "Log in again."})
        with self.assertRaises(NotLoggedIn):
            acetate.ingest(b"x", description, filename="a.flac")
        StandIn.answer = (400, {}, {"code": "rejected", "message": "No title."})
        with self.assertRaises(ServerError) as caught:
            acetate.ingest(b"x", description, filename="a.flac")
        self.assertEqual(caught.exception.code, "rejected")


if __name__ == "__main__":
    unittest.main()
