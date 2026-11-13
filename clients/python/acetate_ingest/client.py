"""Talking to an Acetate server: logging in, and sending files with their descriptions.

`Acetate` blocks; `AsyncAcetate` is the same for asyncio. Both run the same operations:
each is a generator that yields the requests it needs (`_Call`) and is sent their answers,
so what to ask and what the answers mean is written once, and only sending a request
differs between the two.
"""

from __future__ import annotations

import asyncio
import http.client
import io
import json
import os
import secrets
import socket
import ssl
import urllib.parse
from dataclasses import dataclass
from typing import Any, BinaryIO, Generator, Iterator, TypeVar, Union

from . import __version__
from ._shapes import SHAPES
from .description import Description, Image
from .errors import (
    AcetateError,
    CantConnect,
    InvalidDescription,
    NotLoggedIn,
    RateLimited,
    ServerError,
    TooLarge,
    UploadQuotaExceeded,
)
from .report import Report

#: An audio file: a path, its bytes, or an open binary file.
AudioFile = Union[str, "os.PathLike[str]", bytes, BinaryIO]

#: How much is read from a file at a time while sending it.
CHUNK = 1024 * 1024

#: A form part with this type names a chunked upload (`/api/uploads`) instead of carrying
#: a file's bytes.
UPLOAD_PART_TYPE = "application/vnd.acetate.upload"

T = TypeVar("T")
C = TypeVar("C", bound="_Client")


@dataclass
class _Call:
    """One request an operation needs sent."""

    method: str
    path: str
    json_body: Any = None
    multipart: list[_Part] | None = None
    data: bytes | None = None


#: An operation: yields its requests, is sent each one's answer (or has its error thrown
#: in), and returns its result.
_Operation = Generator[_Call, Any, T]


class _Client:
    """What `Acetate` and `AsyncAcetate` share: everything but sending the requests."""

    def __init__(self, url: str, token: str, *, timeout: float = 30 * 60, chunk_size: int = 50_000_000) -> None:
        #: Long by default: a big WAV over a slow link, and the server hashing and probing it.
        self.timeout = timeout
        #: Files bigger than this are sent ahead in pieces of this size, one request each,
        #: under proxies' caps on a request's body (Cloudflare: 100 MB).
        self.chunk_size = chunk_size
        self.url = url.rstrip("/")
        self.token = token

    @classmethod
    def from_env(cls: type[C], **kwargs: Any) -> C:
        """From `ACETATE_URL` and `ACETATE_TOKEN` (as `acetate ingest --server` uses)."""
        url = os.environ.get("ACETATE_URL")
        token = os.environ.get("ACETATE_TOKEN")
        if not url or not token:
            raise InvalidDescription("set ACETATE_URL and ACETATE_TOKEN (the `token` from logging in)")
        return cls(url, token, **kwargs)

    # ---- The operations ----

    def _login(self, username: str, password: str) -> _Operation[None]:
        answer = yield _Call("POST", "/api/auth/login", json_body={"username": username, "password": password})
        self.token = answer["token"]

    def _me(self) -> _Operation[dict[str, Any]]:
        return (yield _Call("GET", "/api/auth/me"))

    def _group_ids(self, names: list[str]) -> _Operation[list[str]]:
        groups = yield _Call("GET", "/api/groups")
        by_name = {g["name"]: g["id"] for g in groups}
        unknown = [n for n in names if n not in by_name]
        if unknown:
            raise InvalidDescription(f"no such group on the server: {', '.join(unknown)} (there are: {', '.join(sorted(by_name))})")
        return [by_name[n] for n in names]

    def _extensions(self) -> _Operation[dict[str, Any]]:
        return (yield _Call("GET", "/api/extensions"))

    def _load_extensions(self) -> _Operation[list[str]]:
        from ._extensions import register

        return register((yield from self._extensions())["extensions"])

    def _source_shapes(self) -> _Operation[list[dict[str, Any]]]:
        shapes = []
        for manifest in (yield from self._extensions())["extensions"]:
            for shape in manifest.get("shape", []):
                shapes.append({"platform": manifest["extension"]["name"], "kind": shape["kind"], "id_field": shape.get("id"), "fields": shape["fields"]})
        return shapes

    def _shape_differences(self) -> _Operation[list[str]]:
        server = {}
        for shape in (yield from self._source_shapes()):
            fields = {f["name"]: (f["type"], f.get("required", False)) for f in shape["fields"]}
            server[(shape["platform"], shape["kind"])] = (shape["id_field"], fields)
        ours = {
            key: (shape.id_field, {name: (f.type, f.required) for name, f in shape.fields.items()})
            for key, shape in SHAPES.items()
        }
        differences = []
        for key in sorted(set(server) | set(ours)):
            name = f"{key[0]} {key[1]}"
            if key not in ours:
                differences.append(f"{name}: only on the server")
            elif key not in server:
                differences.append(f"{name}: only in this library")
            elif ours[key] != server[key]:
                differences.append(f"{name}: different fields (here {ours[key]}, on the server {server[key]})")
        return differences

    def _ingest(
        self,
        file: AudioFile,
        description: Description,
        filename: str | None,
        tags_file: AudioFile | None,
        dry_run: bool,
    ) -> _Operation[Report]:
        images: list[tuple[str, Image]] = []

        def name_image(image: Image) -> str:
            name = f"image-{len(images)}"
            images.append((name, image))
            return name

        if description.metadata is not None and tags_file is not None:
            raise InvalidDescription("a tagged copy (`tags_file`) isn't used with `metadata`: the metadata is given")
        body = description.to_json(name_image)
        parts: list[_Part] = [_Part("description", None, json.dumps(body).encode())]
        parts.append((yield from self._file_part("file", filename or _name_of(file, "file"), _content(file, "file"))))
        if tags_file is not None:
            parts.append((yield from self._file_part("tags_file", _name_of(tags_file, "tags_file", "tagged copy"), _content(tags_file, "tags_file"))))
        for name, image in images:
            parts.append(_Part(name, name, _content(image, name)))

        answer = yield _Call("POST", "/api/ingest" + ("?dry_run=true" if dry_run else ""), multipart=parts)
        return Report.from_json(answer)

    def _file_part(self, name: str, filename: str, content: bytes | str | os.PathLike[str]) -> _Operation[_Part]:
        """A file's form part: its bytes, or for a big file, the chunked upload it was
        sent in first."""
        part = _Part(name, filename, content)
        if part.size() <= self.chunk_size:
            return part
        upload = (yield _Call("POST", "/api/uploads"))["id"]
        offset = 0
        for piece in part.chunks(self.chunk_size):
            tries = 0
            while True:
                try:
                    offset = (yield _Call("PUT", f"/api/uploads/{upload}?offset={offset}", data=piece))["size"]
                    break
                except CantConnect:
                    tries += 1
                    if tries > 3:
                        raise
                    # Go on from what the server has (part of this piece, maybe).
                    got = (yield _Call("GET", f"/api/uploads/{upload}"))["size"]
                    piece, offset = piece[got - offset :], got
                    if not piece:
                        break
        return _Part(name, filename, upload.encode(), kind=UPLOAD_PART_TYPE)

    # ---- HTTP, the parts both ways of sending share ----

    def _prepare(
        self,
        method: str,
        path: str,
        json_body: Any = None,
        multipart: list[_Part] | None = None,
        data: bytes | None = None,
    ) -> tuple[urllib.parse.SplitResult, str, dict[str, str], Iterator[bytes]]:
        """The request's address, target, headers (all but Host) and body, a piece at a time."""
        headers = {"User-Agent": f"acetate-ingest/{__version__} (Python)", "Accept": "application/json"}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        chunks: Iterator[bytes] = iter(())
        if json_body is not None:
            data = json.dumps(json_body).encode()
            headers["Content-Type"] = "application/json"
            headers["Content-Length"] = str(len(data))
            chunks = iter((data,))
        elif data is not None:
            headers["Content-Type"] = "application/octet-stream"
            headers["Content-Length"] = str(len(data))
            chunks = iter((data,))
        elif multipart is not None:
            boundary = "acetate-" + secrets.token_hex(16)
            headers["Content-Type"] = f"multipart/form-data; boundary={boundary}"
            headers["Content-Length"] = str(_multipart_length(multipart, boundary))
            chunks = _multipart_body(multipart, boundary)
        elif method != "GET":
            headers["Content-Length"] = "0"
        url = urllib.parse.urlsplit(self.url + path)
        return url, url.path + (f"?{url.query}" if url.query else ""), headers, chunks

    def _answer(self, response: http.client.HTTPResponse, text: bytes) -> Any:
        if response.status >= 400:
            raise _server_error(response.status, response.reason, response.headers, text)
        return json.loads(text) if text else None


class Acetate(_Client):
    """An Acetate server, logged in.

    >>> acetate = Acetate.login("https://music.example.com", "admin", "...")
    >>> acetate = Acetate("https://music.example.com", token)   # a token from an earlier login
    >>> acetate = Acetate.from_env()                            # ACETATE_URL, ACETATE_TOKEN

    `AsyncAcetate` is the same for asyncio.
    """

    @classmethod
    def login(cls, url: str, username: str, password: str, **kwargs: Any) -> Acetate:
        """Logs in and returns the server with the new session's token (`token`: keep it to
        skip logging in next time; it lasts 30 days unused)."""
        server = cls(url, "", **kwargs)
        server._run(server._login(username, password))
        return server

    # ---- What's there ----

    def me(self) -> dict[str, Any]:
        """Who the token is: `{"username", "permissions", ...}`. A quick check that the
        address and token work."""
        return self._run(self._me())

    def group_ids(self, names: list[str]) -> list[str]:
        """The ids of the server's groups with these names, for `Description.restrict_to`
        (needs `manage_access`)."""
        return self._run(self._group_ids(names))

    def extensions(self) -> dict[str, Any]:
        """The server's extensions (`GET /api/extensions`): each platform's source shapes and links."""
        return self._run(self._extensions())

    def load_extensions(self) -> list[str]:
        """Makes the shapes of the platforms the server has known here, the ones of your own
        too (`RawSource("traktrain", "track", {...})`). Returns the platforms' names."""
        return self._run(self._load_extensions())

    def source_shapes(self) -> list[dict[str, Any]]:
        """The server's source shapes: one for each platform and kind, from its extensions."""
        return self._run(self._source_shapes())

    def shape_differences(self) -> list[str]:
        """How this library's source shapes differ from the server's (empty: the same).
        Anything listed means one of the two is older than the other."""
        return self._run(self._shape_differences())

    # ---- Ingesting ----

    def ingest(
        self,
        file: AudioFile,
        description: Description,
        *,
        filename: str | None = None,
        tags_file: AudioFile | None = None,
        dry_run: bool = False,
    ) -> Report:
        """Sends one audio file with its description, and returns what the server did.

        `file` is stored exactly as sent, never changed. `filename` is the name it's
        recorded as uploaded with (default: the path's file name; needed for bytes and
        files without a name). `tags_file`: a tagged copy of `file` to read the metadata
        from instead (not kept). `dry_run`: work everything out, keep nothing.

        Raises `InvalidDescription` before sending anything the server would refuse, and
        `ServerError` (or one of its kinds: `TooLarge`, `UploadQuotaExceeded`...) when the
        server refuses it. A file the server can't import (e.g. no title or artist in its
        tags) is kept on its Review page, "Didn't import".
        """
        return self._run(self._ingest(file, description, filename, tags_file, dry_run))

    # ---- HTTP ----

    def _run(self, operation: _Operation[T]) -> T:
        answer: Any = None
        error: AcetateError | None = None
        while True:
            try:
                call = operation.send(answer) if error is None else operation.throw(error)
            except StopIteration as done:
                return done.value
            answer, error = None, None
            try:
                answer = self._request(call.method, call.path, json_body=call.json_body, multipart=call.multipart, data=call.data)
            except AcetateError as err:
                error = err

    def _request(
        self,
        method: str,
        path: str,
        *,
        json_body: Any = None,
        multipart: list[_Part] | None = None,
        data: bytes | None = None,
    ) -> Any:
        url, target, headers, chunks = self._prepare(method, path, json_body, multipart, data)
        connection_class = http.client.HTTPSConnection if url.scheme == "https" else http.client.HTTPConnection
        connection = connection_class(url.netloc, timeout=self.timeout)
        try:
            broke: OSError | None = None
            try:
                connection.putrequest(method, target, skip_accept_encoding=True)
                for name, value in headers.items():
                    connection.putheader(name, value)
                connection.endheaders()
                for chunk in chunks:
                    connection.send(chunk)
            except (BrokenPipeError, ConnectionResetError) as err:
                # The server refuses some uploads before reading them (too large, the day's
                # quota used up, not logged in, a description it won't take) and closes the
                # connection; its answer, saying why, is still there to read.
                broke = err
            except OSError as err:
                raise CantConnect(f"can't reach {self.url}: {err}") from None
            try:
                response = connection.getresponse()
                text = response.read()
            except (OSError, http.client.HTTPException) as err:
                raise CantConnect(f"can't reach {self.url}: {broke or err}") from None
        finally:
            connection.close()
        return self._answer(response, text)


class AsyncAcetate(_Client):
    """`Acetate` for asyncio: the same methods, to await.

    >>> acetate = await AsyncAcetate.login("https://music.example.com", "admin", "...")
    >>> acetate = AsyncAcetate.from_env()
    >>> report = await acetate.ingest("Song.flac", description)

    Like `Acetate`, it opens a connection for each request and closes it after, so there's
    nothing to close, and one can be used from any event loop.
    """

    def __init__(self, url: str, token: str, **kwargs: Any) -> None:
        super().__init__(url, token, **kwargs)
        self._tls: ssl.SSLContext | None = None

    @classmethod
    async def login(cls, url: str, username: str, password: str, **kwargs: Any) -> AsyncAcetate:
        """As `Acetate.login`."""
        server = cls(url, "", **kwargs)
        await server._run(server._login(username, password))
        return server

    async def me(self) -> dict[str, Any]:
        """As `Acetate.me`."""
        return await self._run(self._me())

    async def group_ids(self, names: list[str]) -> list[str]:
        """As `Acetate.group_ids`."""
        return await self._run(self._group_ids(names))

    async def extensions(self) -> dict[str, Any]:
        """As `Acetate.extensions`."""
        return await self._run(self._extensions())

    async def load_extensions(self) -> list[str]:
        """As `Acetate.load_extensions`."""
        return await self._run(self._load_extensions())

    async def source_shapes(self) -> list[dict[str, Any]]:
        """As `Acetate.source_shapes`."""
        return await self._run(self._source_shapes())

    async def shape_differences(self) -> list[str]:
        """As `Acetate.shape_differences`."""
        return await self._run(self._shape_differences())

    async def ingest(
        self,
        file: AudioFile,
        description: Description,
        *,
        filename: str | None = None,
        tags_file: AudioFile | None = None,
        dry_run: bool = False,
    ) -> Report:
        """As `Acetate.ingest`. A file given as a path is read a piece at a time while
        sending (each read blocks for as long as reading a megabyte from disk takes)."""
        return await self._run(self._ingest(file, description, filename, tags_file, dry_run))

    # ---- HTTP ----

    async def _run(self, operation: _Operation[T]) -> T:
        answer: Any = None
        error: AcetateError | None = None
        while True:
            try:
                call = operation.send(answer) if error is None else operation.throw(error)
            except StopIteration as done:
                return done.value
            answer, error = None, None
            try:
                answer = await self._request(call.method, call.path, json_body=call.json_body, multipart=call.multipart, data=call.data)
            except AcetateError as err:
                error = err

    async def _request(
        self,
        method: str,
        path: str,
        *,
        json_body: Any = None,
        multipart: list[_Part] | None = None,
        data: bytes | None = None,
    ) -> Any:
        url, target, headers, chunks = self._prepare(method, path, json_body, multipart, data)
        # Read to the end of the connection rather than by the response's framing.
        headers["Connection"] = "close"
        head = f"{method} {target} HTTP/1.1\r\nHost: {url.netloc}\r\n"
        head += "".join(f"{name}: {value}\r\n" for name, value in headers.items()) + "\r\n"
        tls = None
        if url.scheme == "https":
            if self._tls is None:
                self._tls = ssl.create_default_context()
                self._tls.set_alpn_protocols(["http/1.1"])
            tls = self._tls
        connection = _AsyncConnection(self.timeout)
        try:
            broke: OSError | None = None
            try:
                await connection.open(url.hostname or "", url.port or (443 if tls else 80), tls)
                await connection.send(head.encode("latin-1"))
                for chunk in chunks:
                    await connection.send(chunk)
            except (BrokenPipeError, ConnectionResetError) as err:
                # As in Acetate._request: the server's answer to an upload it refused
                # before reading it is still there to read.
                broke = err
            except OSError as err:
                raise CantConnect(f"can't reach {self.url}: {err}") from None
            try:
                received = await connection.receive_all()
                response = http.client.HTTPResponse(_Received(received), method=method)  # type: ignore[arg-type]
                response.begin()
                text = response.read()
            except (OSError, http.client.HTTPException) as err:
                raise CantConnect(f"can't reach {self.url}: {broke or err}") from None
        finally:
            connection.close()
        return self._answer(response, text)


class _AsyncConnection:
    """One connection, on a non-blocking socket used directly (TLS through memory BIOs).

    Not asyncio's streams: when a write fails because the server closed the connection,
    their transport is torn down and whatever the server answered before closing, still
    unread in the socket, is lost. Here it's still read afterwards, as `http.client` does.
    """

    def __init__(self, timeout: float) -> None:
        self.timeout = timeout
        self.sock: socket.socket | None = None
        self.tls: ssl.SSLObject | None = None
        self.incoming = ssl.MemoryBIO()
        self.outgoing = ssl.MemoryBIO()

    async def _within(self, awaitable: Any) -> Any:
        try:
            return await asyncio.wait_for(awaitable, self.timeout)
        except asyncio.TimeoutError:
            # An OSError, as the blocking socket's timeout is (asyncio's isn't one before 3.11).
            raise TimeoutError("timed out") from None

    async def open(self, host: str, port: int, tls: ssl.SSLContext | None) -> None:
        loop = asyncio.get_running_loop()
        addresses = await self._within(loop.getaddrinfo(host, port, type=socket.SOCK_STREAM))
        error: OSError | None = None
        for family, kind, proto, _, address in addresses:
            sock = socket.socket(family, kind, proto)
            sock.setblocking(False)
            try:
                await self._within(loop.sock_connect(sock, address))
            except OSError as err:
                sock.close()
                error = err
                continue
            except BaseException:
                sock.close()
                raise
            self.sock = sock
            break
        if self.sock is None:
            raise error or OSError(f"no address for {host}")
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        if tls is not None:
            self.tls = tls.wrap_bio(self.incoming, self.outgoing, server_hostname=host)
            while True:
                try:
                    self.tls.do_handshake()
                    break
                except ssl.SSLWantReadError:
                    await self._flush()
                    if not await self._fill():
                        raise ConnectionResetError("the connection closed during the TLS handshake") from None
            await self._flush()

    async def send(self, data: bytes) -> None:
        assert self.sock is not None
        if self.tls is None:
            await self._within(asyncio.get_running_loop().sock_sendall(self.sock, data))
            return
        self.tls.write(data)
        await self._flush()

    async def receive_all(self) -> bytes:
        """Everything until the server closes the connection. A connection reset after
        some of it: what came (if the answer isn't all there, reading it says so)."""
        received = bytearray()
        try:
            while data := await self._receive():
                received += data
        except OSError:
            if not received:
                raise
        return bytes(received)

    async def _receive(self) -> bytes:
        assert self.sock is not None
        if self.tls is None:
            return await self._within(asyncio.get_running_loop().sock_recv(self.sock, CHUNK))
        while True:
            try:
                return self.tls.read(CHUNK)
            except ssl.SSLWantReadError:
                if not await self._fill():
                    self.incoming.write_eof()
            except (ssl.SSLZeroReturnError, ssl.SSLEOFError):
                # Closed, with or without saying so first (as `ssl` sockets take either).
                return b""

    async def _fill(self) -> bool:
        """Reads what's come into the TLS layer; False at the end of the connection."""
        assert self.sock is not None
        data = await self._within(asyncio.get_running_loop().sock_recv(self.sock, CHUNK))
        if data:
            self.incoming.write(data)
        return bool(data)

    async def _flush(self) -> None:
        assert self.sock is not None
        if self.outgoing.pending:
            await self._within(asyncio.get_running_loop().sock_sendall(self.sock, self.outgoing.read()))

    def close(self) -> None:
        if self.sock is not None:
            self.sock.close()


class _Received:
    """A response already read, as `http.client.HTTPResponse` reads one from a socket."""

    def __init__(self, data: bytes) -> None:
        self.data = data

    def makefile(self, mode: str) -> io.BytesIO:
        return io.BytesIO(self.data)


def _server_error(status: int, reason: str, headers: Any, text: bytes) -> ServerError:
    try:
        body = json.loads(text or b"{}")
    except ValueError:
        body = {}
    if not isinstance(body, dict):
        body = {}
    code = body.get("code") or "unknown"
    message = body.get("message") or f"{status} {reason}"
    retry_after = headers.get("Retry-After")
    retry = int(retry_after) if retry_after and retry_after.isdigit() else None
    if status == 401:
        return NotLoggedIn(status, code, message)
    if status == 413:
        return TooLarge(status, code, message)
    if status == 429 and code == "upload_quota":
        return UploadQuotaExceeded(status, code, message, retry)
    if status == 429:
        return RateLimited(status, code, message, retry)
    return ServerError(status, code, message)


# ---- Multipart, streamed: files are read a piece at a time while sending ----


class _Part:
    def __init__(self, name: str, filename: str | None, content: bytes | str | os.PathLike[str], kind: str | None = None) -> None:
        self.name = name
        self.filename = filename
        self.content = content
        self.kind = kind

    def head(self, boundary: str) -> bytes:
        disposition = f'form-data; name="{_quote(self.name)}"'
        if self.filename is not None:
            disposition += f'; filename="{_quote(self.filename)}"'
        kind = self.kind or ("application/json" if self.name == "description" else "application/octet-stream")
        return f"--{boundary}\r\nContent-Disposition: {disposition}\r\nContent-Type: {kind}\r\n\r\n".encode()

    def size(self) -> int:
        if isinstance(self.content, bytes):
            return len(self.content)
        return os.path.getsize(self.content)

    def chunks(self, size: int = CHUNK) -> Iterator[bytes]:
        if isinstance(self.content, bytes):
            for start in range(0, len(self.content), size):
                yield self.content[start : start + size]
            return
        with open(self.content, "rb") as f:
            while chunk := f.read(size):
                yield chunk


def _quote(text: str) -> str:
    # As browsers do: quotes and line breaks escaped, the rest (UTF-8 names too) as is.
    return text.replace("\\", "\\\\").replace('"', "%22").replace("\r", "%0D").replace("\n", "%0A")


def _multipart_length(parts: list[_Part], boundary: str) -> int:
    return sum(len(p.head(boundary)) + p.size() + 2 for p in parts) + len(f"--{boundary}--\r\n")


def _multipart_body(parts: list[_Part], boundary: str) -> Iterator[bytes]:
    for part in parts:
        yield part.head(boundary)
        yield from part.chunks()
        yield b"\r\n"
    yield f"--{boundary}--\r\n".encode()


def _content(file: Any, what: str) -> bytes | str | os.PathLike[str]:
    """A file's content as a part takes it: bytes as they are, a path to read from while
    sending, an open file read now."""
    if isinstance(file, (bytes, bytearray)):
        return bytes(file)
    if isinstance(file, (str, os.PathLike)):
        if not os.path.isfile(file):
            raise InvalidDescription(f"{what}: no such file: {os.fspath(file)}")
        return file
    if isinstance(file, io.IOBase) or hasattr(file, "read"):
        data = file.read()
        if not isinstance(data, bytes):
            raise InvalidDescription(f"{what}: open the file in binary mode ('rb')")
        return data
    raise InvalidDescription(f"{what}: {file!r} isn't a file (a path, bytes, or an open binary file)")


def _name_of(file: Any, what: str, default: str | None = None) -> str:
    if isinstance(file, (str, os.PathLike)):
        return os.path.basename(os.fspath(file))
    name = getattr(file, "name", None)
    if isinstance(name, str) and name:
        return os.path.basename(name)
    if default is not None:
        return default
    raise InvalidDescription(f"{what}: give `filename=` (the name it was downloaded as): it's recorded with the file")
