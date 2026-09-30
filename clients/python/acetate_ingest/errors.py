"""What can go wrong. Every error the library raises is an `AcetateError`."""

from __future__ import annotations


class AcetateError(Exception):
    """Anything that went wrong talking to Acetate."""


class InvalidDescription(AcetateError, ValueError):
    """A description or source that the server would refuse, found before sending it.
    Nothing was sent."""


class CantConnect(AcetateError):
    """The server couldn't be reached (wrong address, it's down, the network)."""


class ServerError(AcetateError):
    """The server refused the request or failed. `message` is a sentence for a person
    (show it as it is); `code` a stable name for code to act on (`invalid`, `rejected`,
    `forbidden`, `not_found`, `conflict`, `internal`...); `status` the HTTP status."""

    def __init__(self, status: int, code: str, message: str) -> None:
        super().__init__(message)
        self.status = status
        self.code = code
        self.message = message

    def __repr__(self) -> str:
        return f"{type(self).__name__}({self.status}, {self.code!r}, {self.message!r})"


class NotLoggedIn(ServerError):
    """401: the token is wrong, expired or logged out. Log in again."""


class TooLarge(ServerError):
    """413: the request is over the server's size limit per request (file, tagged copy and
    covers together). Such a file has to be imported on the server (`acetate ingest`)."""


class UploadQuotaExceeded(ServerError):
    """429: the server's uploads for today are used up (all accounts together). Stop
    sending; it starts again at midnight UTC, in `retry_after` seconds."""

    def __init__(self, status: int, code: str, message: str, retry_after: int | None) -> None:
        super().__init__(status, code, message)
        self.retry_after = retry_after


class RateLimited(ServerError):
    """429 for anything else (e.g. too many failed logins): wait `retry_after` seconds."""

    def __init__(self, status: int, code: str, message: str, retry_after: int | None) -> None:
        super().__init__(status, code, message)
        self.retry_after = retry_after
