"""`AsyncAcetate` against the same stand-in as test_ingest.py, and chunked uploads both ways."""

import asyncio
import email.parser
import email.policy
import http.server
import json
import os
import socket
import tempfile
import threading
import unittest

from acetate_ingest import (
    Acetate,
    AlbumHint,
    AsyncAcetate,
    CantConnect,
    Description,
    InvalidDescription,
    NetEaseTrack,
    NotLoggedIn,
    ServerError,
    TooLarge,
    UploadQuotaExceeded,
)

from .test_ingest import StandIn

REPORT = {"status": "created", "track_id": "t1", "dry_run": False}


def serve(handler):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}"


class AsyncSending(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        cls.server, cls.url = serve(StandIn)

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def setUp(self):
        StandIn.got.clear()

    async def test_a_file_its_description_and_covers(self):
        StandIn.answer = (200, {}, dict(REPORT, dry_run=True))
        with tempfile.TemporaryDirectory() as folder:
            audio = os.path.join(folder, "Ravel. - Song.flac")
            with open(audio, "wb") as f:
                f.write(b"fLaC" + os.urandom(3 * 1024 * 1024))  # more than one chunk
            with open(audio, "rb") as f:
                sent_audio = f.read()
            report = await AsyncAcetate(self.url, "tok").ingest(
                audio,
                Description(origin=NetEaseTrack(song_id=1), album=AlbumHint(covers=[b"\xff\xd8jpeg"])),
                tags_file=b"tagged",
                dry_run=True,
            )

        self.assertTrue(report.created and report.dry_run)
        path, headers, body = StandIn.got[0]
        self.assertEqual(path, "/api/ingest?dry_run=true")
        self.assertEqual(headers["Authorization"], "Bearer tok")
        message = email.parser.BytesParser(policy=email.policy.HTTP).parsebytes(
            b"Content-Type: " + headers["Content-Type"].encode() + b"\r\n\r\n" + body
        )
        parts = {p.get_param("name", header="content-disposition"): p for p in message.iter_parts()}
        self.assertEqual(list(parts), ["description", "file", "tags_file", "image-0"])
        self.assertEqual(json.loads(parts["description"].get_content())["album"]["covers"], ["image-0"])
        self.assertEqual(parts["file"].get_filename(), "Ravel. - Song.flac")
        self.assertEqual(parts["file"].get_content(), sent_audio)
        self.assertEqual(parts["tags_file"].get_content(), b"tagged")
        self.assertEqual(parts["image-0"].get_content(), b"\xff\xd8jpeg")

    async def test_the_event_loop_runs_while_sending(self):
        StandIn.answer = (200, {}, REPORT)
        turns = 0
        sending = True

        async def count():
            nonlocal turns
            while sending:
                turns += 1
                await asyncio.sleep(0)

        counter = asyncio.create_task(count())
        await AsyncAcetate(self.url, "tok").ingest(
            os.urandom(8 * 1024 * 1024), Description(origin=NetEaseTrack(song_id=1)), filename="a.flac"
        )
        sending = False
        await counter
        self.assertGreater(turns, 1)

    async def test_bytes_need_a_name(self):
        with self.assertRaisesRegex(InvalidDescription, "filename="):
            await AsyncAcetate(self.url, "tok").ingest(b"audio", Description(origin=NetEaseTrack(song_id=1)))
        self.assertEqual(StandIn.got, [], "nothing sent")

    async def test_errors_carry_the_servers_message(self):
        acetate = AsyncAcetate(self.url, "tok")
        description = Description(origin=NetEaseTrack(song_id=1))
        StandIn.answer = (429, {"Retry-After": "3600"}, {"code": "upload_quota", "message": "Today's uploads are used up."})
        with self.assertRaises(UploadQuotaExceeded) as caught:
            await acetate.ingest(b"x", description, filename="a.flac")
        self.assertEqual((caught.exception.retry_after, str(caught.exception)), (3600, "Today's uploads are used up."))
        StandIn.answer = (401, {}, {"code": "unauthorized", "message": "Log in again."})
        with self.assertRaises(NotLoggedIn):
            await acetate.ingest(b"x", description, filename="a.flac")
        StandIn.answer = (400, {}, {"code": "rejected", "message": "No title."})
        with self.assertRaises(ServerError) as caught:
            await acetate.ingest(b"x", description, filename="a.flac")
        self.assertEqual(caught.exception.code, "rejected")

    async def test_login(self):
        StandIn.answer = (200, {}, {"token": "new"})
        acetate = await AsyncAcetate.login(self.url, "admin", "pw")
        self.assertEqual(acetate.token, "new")
        path, _, body = StandIn.got[0]
        self.assertEqual((path, json.loads(body)), ("/api/auth/login", {"username": "admin", "password": "pw"}))

    async def test_a_refusal_before_the_upload_is_read(self):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()

        def refuse():
            conn, _ = listener.accept()
            conn.recv(65536)
            body = json.dumps({"code": "too_large", "message": "At most 100 MB at a time."}).encode()
            conn.sendall(
                b"HTTP/1.1 413 Payload Too Large\r\nConnection: close\r\nContent-Length: %d\r\n\r\n%s"
                % (len(body), body)
            )
            conn.shutdown(socket.SHUT_WR)
            conn.close()

        threading.Thread(target=refuse, daemon=True).start()
        url = f"http://127.0.0.1:{listener.getsockname()[1]}"
        try:
            with self.assertRaises(TooLarge) as caught:
                await AsyncAcetate(url, "tok").ingest(
                    b"x" * 50_000_000, Description(origin=NetEaseTrack(song_id=1)), filename="a.flac"
                )
        finally:
            listener.close()
        self.assertEqual(str(caught.exception), "At most 100 MB at a time.")

    async def test_no_server(self):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
        listener.close()
        with self.assertRaises(CantConnect):
            await AsyncAcetate(f"http://127.0.0.1:{port}", "tok").me()

    async def test_extensions_are_loaded(self):
        from acetate_ingest import SHAPES, RawSource

        class Extensions(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                manifest = {"extension": {"name": "beats"}, "shape": [{"kind": "track", "id": "track_id", "fields": [{"name": "track_id", "type": "int", "required": True}]}]}
                data = json.dumps({"extensions": [manifest]}).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        server, url = serve(Extensions)
        try:
            self.assertEqual(await AsyncAcetate(url, "tok").load_extensions(), ["beats"])
            self.assertEqual(RawSource("beats", "track", {"track_id": 1}).data(), {"track_id": 1})
        finally:
            SHAPES.pop(("beats", "track"), None)
            await asyncio.to_thread(server.shutdown)
            server.server_close()


class Uploads(http.server.BaseHTTPRequestHandler):
    """The chunked-upload endpoints; drops the first piece's connection without answering."""

    received = bytearray()
    drop_next = True
    ingested = []

    def answer(self, body):
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def body(self):
        return self.rfile.read(int(self.headers["Content-Length"]))

    def do_POST(self):
        body = self.body()
        if self.path == "/api/uploads":
            self.answer({"id": "u1"})
        else:
            Uploads.ingested.append(body)
            self.answer(REPORT)

    def do_PUT(self):
        offset = int(self.path.split("offset=")[1])
        piece = self.body()
        if Uploads.drop_next:
            # Half the piece arrived, then the connection broke.
            Uploads.drop_next = False
            Uploads.received[offset:] = piece[: len(piece) // 2]
            self.close_connection = True
            self.connection.shutdown(socket.SHUT_RDWR)
            return
        Uploads.received[offset:] = piece
        self.answer({"size": len(Uploads.received)})

    def do_GET(self):
        self.answer({"size": len(Uploads.received)})

    def log_message(self, *args):
        pass


class ChunkedUploads(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        cls.server, cls.url = serve(Uploads)

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def setUp(self):
        Uploads.received = bytearray()
        Uploads.drop_next = True
        Uploads.ingested = []

    def check(self, audio, report):
        self.assertTrue(report.created)
        self.assertEqual(bytes(Uploads.received), audio, "a dropped piece goes on from what the server has")
        self.assertIn(b"application/vnd.acetate.upload\r\n\r\nu1\r\n", Uploads.ingested[0])

    async def test_async(self):
        audio = os.urandom(50_000)
        report = await AsyncAcetate(self.url, "tok", chunk_size=20_000).ingest(
            audio, Description(origin=NetEaseTrack(song_id=1)), filename="a.flac"
        )
        self.check(audio, report)

    def test_sync(self):
        audio = os.urandom(50_000)
        report = Acetate(self.url, "tok", chunk_size=20_000).ingest(
            audio, Description(origin=NetEaseTrack(song_id=1)), filename="a.flac"
        )
        self.check(audio, report)


if __name__ == "__main__":
    unittest.main()
