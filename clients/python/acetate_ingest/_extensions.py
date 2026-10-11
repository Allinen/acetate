"""Platforms that aren't built in: extensions the server has (or a folder of them).

The typed sources (`NetEaseTrack`...) are made from the extensions that come with Acetate.
A platform of your own (an `extension.toml` in the server's `ACETATE_EXTENSIONS_DIR`) is
used as a `RawSource("traktrain", "track", {...})`, checked like the typed ones once its
shapes are known here: `Acetate.load_extensions()` asks the server for them, or
`load_extension_files("path/to/extensions")` reads the same files.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from ._shapes import SHAPES
from ._source import Field, Shape


def register(manifests: list[dict[str, Any]]) -> list[str]:
    """Makes the shapes of these extensions (as the server serves them, or an
    `extension.toml` read) known. Returns the platforms' names."""
    names = []
    for manifest in manifests:
        name = manifest["extension"]["name"]
        names.append(name)
        for shape in manifest.get("shape", []):
            fields = {f["name"]: Field(f["type"], bool(f.get("required", False))) for f in shape["fields"]}
            SHAPES[(name, shape["kind"])] = Shape(id_field=shape.get("id"), fields=fields)
    return names


def load_extension_files(folder: str | Path) -> list[str]:
    """Reads `*/extension.toml` in `folder` (what the server's `ACETATE_EXTENSIONS_DIR` has) and
    makes their shapes known. Needs Python 3.11 (`tomllib`). Returns the platforms' names."""
    try:
        import tomllib
    except ImportError:
        raise RuntimeError("reading extension.toml needs Python 3.11 or newer: use Acetate.load_extensions() instead") from None
    manifests = []
    for file in sorted(Path(folder).glob("*/extension.toml")):
        with open(file, "rb") as f:
            manifests.append(tomllib.load(f))
    return register(manifests)
