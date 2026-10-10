#!/usr/bin/env python3
"""Acetate's version, in the places that have to agree.

    scripts/versions.py                  # check them (the hooks and CI run this)
    scripts/versions.py --tag v0.2.0     # also: the release tag says the same (the release workflow)
    scripts/versions.py --set 0.2.0      # write the version to all of them

The server, the core, the desktop app, the Android app and the Python library are released
together under one version: the apps ask the server theirs and say so when it isn't their
own. `--set` also refreshes the Cargo.lock files. Also checked: API_VERSION and
CATALOG_FORMAT hold a number, and docs/keys.md is what the keymap says.
"""
import re
import subprocess
import sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent

# (file, the line that has the version, with the version as group 2)
PLACES = [
    ("Cargo.toml", r'^(version = ")([^"]+)(")', re.M),
    ("clients/core/Cargo.toml", r'^(version = ")([^"]+)(")', re.M),
    ("clients/desktop/Cargo.toml", r'^(version = ")([^"]+)(")', re.M),
    ("clients/android/rust/Cargo.toml", r'^(version = ")([^"]+)(")', re.M),
    ("clients/python/pyproject.toml", r'^(version = ")([^"]+)(")', re.M),
    ("clients/python/acetate_ingest/__init__.py", r'^(__version__ = ")([^"]+)(")', re.M),
    # What a build without -Pacetate.versionName calls itself.
    ("clients/android/app/build.gradle.kts", r'(versionName = \(findProperty\("acetate.versionName"\) as String\?\) \?: ")([^"]+)(")', 0),
]
VERSION = re.compile(r"\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$")


def read(path, pattern, flags):
    found = re.search(pattern, (root / path).read_text(), flags)
    if not found:
        sys.exit(f"{path}: can't find the version")
    return found.group(2)


def current():
    return {path: read(path, pattern, flags) for path, pattern, flags in PLACES}


def check(tag):
    problems = []
    versions = current()
    first = versions["Cargo.toml"]
    for path, version in versions.items():
        if version != first:
            problems.append(f"{path} says {version}, Cargo.toml says {first}")
    if not VERSION.match(first):
        problems.append(f"{first} isn't a version like 1.2.3")
    if tag and tag != f"v{first}":
        problems.append(f"the tag is {tag}, the version is {first}")
    # The lock files keep the workspace's own versions; a stale one means a build changed them.
    for lock in ["Cargo.lock", "clients/Cargo.lock"]:
        for name, version in re.findall(r'name = "(acetate[a-z-]*)"\nversion = "([^"]+)"', (root / lock).read_text()):
            if version != first:
                problems.append(f"{lock}: {name} is {version}")
    for name in ["API_VERSION", "CATALOG_FORMAT"]:
        if not (root / name).read_text().strip().isdigit():
            problems.append(f"{name} should hold a number")
    keys = subprocess.run([sys.executable, str(root / "scripts/gen-keys-doc.py"), "--check"], capture_output=True, text=True)
    if keys.returncode:
        problems.append(keys.stderr.strip())
    if problems:
        sys.exit("\n".join(problems))
    print(f"all agree on {first}")


def write(version):
    if not VERSION.match(version):
        sys.exit(f"{version} isn't a version like 1.2.3")
    for path, pattern, flags in PLACES:
        text = (root / path).read_text()
        (root / path).write_text(re.sub(pattern, lambda m: m.group(1) + version + m.group(3), text, count=1, flags=flags))
    for workspace in [".", "clients"]:
        subprocess.run(["cargo", "update", "--workspace", "--offline"], cwd=root / workspace, check=True)
    check(None)


args = sys.argv[1:]
if args[:1] == ["--set"] and len(args) == 2:
    write(args[1].removeprefix("v"))
elif args[:1] == ["--tag"] and len(args) == 2:
    check(args[1])
elif not args:
    check(None)
else:
    sys.exit(__doc__)
