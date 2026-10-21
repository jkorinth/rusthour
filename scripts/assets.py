#!/usr/bin/env python3
"""Helpers for the `assets` workflow (.github/workflows/assets.yml).

  assets.py plan AREAS_TOML NAMES
      Print "<area> <W,S,E,N>" per requested area (comma-separated NAMES,
      empty = all). Exits non-zero on an invalid or unknown name.
  assets.py record MANIFEST KEY=VALUE... FILE...
      Append one build (the KEY=VALUE fields, then each FILE with its sha256)
      to MANIFEST (data/assets.toml).
"""

import hashlib
import json
import pathlib
import re
import sys
import tomllib

NAME = re.compile(r"[a-z0-9-]+")


def osmium_bbox(bbox):
    """areas.toml has [south, west, north, east]; osmium -b wants W,S,E,N."""
    south, west, north, east = bbox
    return f"{west},{south},{east},{north}"


def plan(areas_toml, names):
    areas = tomllib.loads(pathlib.Path(areas_toml).read_text())["areas"]
    wanted = [n.strip() for n in names.split(",")] if names.strip() else list(areas)
    result = []
    for name in dict.fromkeys(wanted):
        if not NAME.fullmatch(name) or name not in areas:
            sys.exit(f"error: unknown area {name!r}; known: {', '.join(areas)}")
        result.append((name, osmium_bbox(areas[name]["bbox"])))
    return result


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def record(manifest, fields, files):
    # Hand-written TOML (tomllib can't write); json.dumps gives valid TOML strings.
    lines = ["", "[[builds]]"] + [f"{k} = {json.dumps(v)}" for k, v in fields.items()]
    for f in map(pathlib.Path, files):
        lines += ["", "[[builds.files]]", f"area = {json.dumps(f.name.removesuffix('.osm.pbf'))}",
                  f"file = {json.dumps(f.name)}", f"sha256 = {json.dumps(sha256(f))}"]
    with open(manifest, "a") as out:
        out.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    cmd, *args = sys.argv[1:] or ["help"]
    if cmd == "plan" and len(args) == 2:
        for name, bbox in plan(*args):
            print(name, bbox)
    elif cmd == "record" and len(args) >= 2:
        fields = dict(a.split("=", 1) for a in args[1:] if "=" in a)
        record(args[0], fields, [a for a in args[1:] if "=" not in a])
    else:
        sys.exit(__doc__)
