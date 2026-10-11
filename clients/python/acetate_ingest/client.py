"""Talking to an Acetate server: logging in, and sending files with their descriptions."""

from __future__ import annotations

import http.client
import io
import json
import os
import secrets
import urllib.parse
from typing import Any, BinaryIO, Iterator, Union

from . import __version__
from ._shapes import SHAPES
from .description import Description, Image
from .errors import (
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


class Acetate:
    """An Acetate server, logged in.

    >>> acetate = Acetate.login("https://music.example.com", "admin", "...")
    >>> acetate = Acetate("https://music.example.com", token)   # a token from an earlier login
    >>> acetate = Acetate.from_env()                            # ACETATE_URL, ACETATE_TOKEN
    """

    def __init__(self, url: str, token: str, *, timeout: float = 30 * 60, chunk_size: int = 50_000_000) -> None:
        #: Long by default: a big WAV over a slow link, and the server hashing and probing it.
        self.timeout = timeout
        #: Files bigger than this are sent ahead in pieces of this size, one request each,
        #: under proxies' caps on a request's body (Cloudflare: 100 MB).
        self.chunk_size = chunk_size
        self.url = url.rstrip("/")
        self.token = token

    @classmethod
    def login(cls, url: str, username: str, password: str, **kwargs: Any) -> Acetate:
        """Logs in and returns the server with the new session's token (`token`: keep it to
        skip logging in next time; it lasts 30 days unused)."""
        server = cls(url, "", **kwargs)
        answer = server._request("POST", "/api/auth/login", json_body={"username": username, "password": password})
        server.token = answer["token"]
        return server

    @classmethod
    def from_env(cls, **kwargs: Any) -> Acetate:
        """From `ACETATE_URL` and `ACETATE_TOKEN` (as `acetate ingest --server` uses)."""
        url = os.environ.get("ACETATE_URL")
        token = os.environ.get("ACETATE_TOKEN")
        if not url or not token:
            raise InvalidDescription("set ACETATE_URL and ACETATE_TOKEN (the `token` from logging in)")
        return cls(url, token, **kwargs)

    # ---- What's there ----

    def me(self) -> dict[str, Any]:
        """Who the token is: `{"username", "permissions", ...}`. A quick check that the
        address and token work."""
        return self._request("GET", "/api/auth/me")

    def group_ids(self, names: list[str]) -> list[str]:
        """The ids of the server's groups with these names, for `Description.restrict_to`
        (needs `manage_access`)."""
        groups = self._request("GET", "/api/groups")
        by_name = {g["name"]: g["id"] for g in groups}
        unknown = [n for n in names if n not in by_name]
        if unknown:
            raise InvalidDescription(f"no such group on the server: {', '.join(unknown)} (there are: {', '.join(sorted(by_name))})")
        return [by_name[n] for n in names]

    def extensions(self) -> dict[str, Any]:
        """The server's extensions (`GET /api/extensions`): each platform's source shapes and links."""
        return self._request("GET", "/api/extensions")

    def source_shapes(self) -> list[dict[str, Any]]:
        """The server's source shapes: one for each platform and kind, from its extensions."""
        shapes = []
        for manifest in self.extensions()["extensions"]:
            for shape in manifest.get("shape", []):
                shapes.append({"platform": manifest["extension"]["name"], "kind": shape["kind"], "id_field": shape.get("id"), "fields": shape["fields"]})
        return shapes

    def shape_differences(self) -> list[str]:
        """How this library's source shapes differ from the server's (empty: the same).
        Anything listed means one of the two is older than the other."""
        server = {}
        for shape in self.source_shapes():
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
        images: list[tuple[str, Image]] = []

        def name_image(image: Image) -> str:
            name = f"image-{len(images)}"
            images.append((name, image))
            return name

        if description.metadata is not None and tags_file is not None:
            raise InvalidDescription("a tagged copy (`tags_file`) isn't used with `metadata`: the metadata is given")
        body = description.to_json(name_image)
        parts: list[_Part] = [_Part("description", None, json.dumps(body).encode())]
        parts.append(self._file_part("file", filename or _name_of(file, "file"), _content(file, "file")))
        if tags_file is not None:
            parts.append(self._file_part("tags_file", _name_of(tags_file, "tags_file", "tagged copy"), _content(tags_file, "tags_file")))
        for name, image in images:
            parts.append(_Part(name, name, _content(image, name)))

        answer = self._request("POST", "/api/ingest" + ("?dry_run=true" if dry_run else ""), multipart=parts)
        return Report.from_json(answer)

    def _file_part(self, name: str, filename: str, content: bytes | str | os.PathLike[str]) -> _Part:
        """A file's form part: its bytes, or for a big file, the chunked upload it was
        sent in first."""
        part = _Part(name, filename, content)
        if part.size() <= self.chunk_size:
            return part
        upload = self._request("POST", "/api/uploads")["id"]
        offset = 0
        for piece in part.chunks(self.chunk_size):
            tries = 0
            while True:
                try:
                    offset = self._request("PUT", f"/api/uploads/{upload}?offset={offset}", data=piece)["size"]
                    break
                except CantConnect:
                    tries += 1
                    if tries > 3:
                        raise
                    # Go on from what the server has (part of this piece, maybe).
                    got = self._request("GET", f"/api/uploads/{upload}")["size"]
                    piece, offset = piece[got - offset :], got
                    if not piece:
                        break
        return _Part(name, filename, upload.encode(), kind=UPLOAD_PART_TYPE)

    # ---- HTTP ----

    def _request(
        self,
        method: str,
        path: str,
        *,
        json_body: Any = None,
        multipart: list[_Part] | None = None,
        data: bytes | None = None,
    ) -> Any:
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
        connection_class = http.client.HTTPSConnection if url.scheme == "https" else http.client.HTTPConnection
        connection = connection_class(url.netloc, timeout=self.timeout)
        try:
            broke: OSError | None = None
            try:
                connection.putrequest(method, url.path + (f"?{url.query}" if url.query else ""), skip_accept_encoding=True)
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
        if response.status >= 400:
            raise _server_error(response.status, response.reason, response.headers, text)
        return json.loads(text) if text else None


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
