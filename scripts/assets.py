#!/usr/bin/env python3
"""Helpers for the `assets` workflow (.github/workflows/assets.yml).

  assets.py plan AREAS_TOML NAMES
      Print "<area> <W,S,E,N>" per requested area (comma-separated NAMES,
      empty = all). Exits non-zero on an invalid or unknown name.
  assets.py record MANIFEST KEY=VALUE... FILE...
      Append one build (the KEY=VALUE fields, then each FILE with its sha256)
      to MANIFEST (data/assets.toml).
  assets.py hash-rename FILE...
      Rename each <area>.rhnet to <area>.<sha256>.rhnet (ADR-0002).
  assets.py schema
      Print the NETWORK_SCHEMA_VERSION of this checkout.
  assets.py bundle MANIFEST DIR
      Per area, download the .rhnet map of the newest build in MANIFEST that
      matches this checkout's schema into DIR (via `gh`; needs GH_TOKEN and
      GITHUB_REPOSITORY), verify it and write DIR/manifest.json for the
      viewer. No maps: manifest.json is `[]`.
"""

import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tomllib

NAME = re.compile(r"[a-z0-9-]+")
TAG = re.compile(r"assets-[0-9-]+")
NETWORK_RS = pathlib.Path(__file__).resolve().parent.parent / "crates/rusthour-api/src/network.rs"


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
        kind = "rhnet" if f.suffix == ".rhnet" else "osm.pbf"
        # Area names have no dots: <area>.osm.pbf or <area>.<sha256>.rhnet.
        lines += ["", "[[builds.files]]", f"kind = {json.dumps(kind)}",
                  f"area = {json.dumps(f.name.split('.')[0])}",
                  f"file = {json.dumps(f.name)}", f"sha256 = {json.dumps(sha256(f))}"]
    with open(manifest, "a") as out:
        out.write("\n".join(lines) + "\n")


def hash_rename(path):
    path = pathlib.Path(path)
    return path.rename(path.with_name(f"{path.stem}.{sha256(path)}.rhnet"))


def schema_version():
    found = re.search(r"pub const NETWORK_SCHEMA_VERSION: u32 = (\d+);", NETWORK_RS.read_text())
    if not found:
        sys.exit(f"error: no NETWORK_SCHEMA_VERSION in {NETWORK_RS}")
    return int(found[1])


def rhnet_files(build):
    return [f for f in build.get("files", []) if f.get("kind") == "rhnet"]


def check(build, f):
    """Exit unless the tag and file name are safe as `gh` arguments and paths."""
    area, digest = f.get("area", ""), f.get("sha256", "")
    if not TAG.fullmatch(build.get("tag", "")) or not NAME.fullmatch(area) \
            or not re.fullmatch(r"[0-9a-f]{64}", digest) or f.get("file") != f"{area}.{digest}.rhnet":
        sys.exit(f"error: bad map entry in build {build.get('tag')!r}: {f}")


def select_maps(builds, schema):
    """Per area, the newest build's map of `schema`: ([(build, file)], skipped newer tags)."""
    # Newest by source timestamp; equal timestamps: the later entry in the file.
    ordered = sorted(enumerate(builds), key=lambda ib: (ib[1].get("source_timestamp", ""), ib[0]),
                     reverse=True)
    chosen, skipped = {}, {}
    for _, build in ordered:
        for f in rhnet_files(build):
            check(build, f)
            if f["area"] in chosen:
                continue
            if build.get("network_schema_version") == schema:
                chosen[f["area"]] = (build, f)
            else:
                skipped[build["tag"]] = None
    return [chosen[a] for a in sorted(chosen)], list(skipped)


def maps_manifest(chosen, folder):
    """manifest.json entries for the `chosen` maps, already downloaded to `folder`."""
    entries = []
    for build, f in chosen:
        path = pathlib.Path(folder, f["file"])
        if sha256(path) != f["sha256"]:
            sys.exit(f"error: {f['file']} from {build['tag']} doesn't match its sha256")
        entries.append({"area": f["area"], "file": f["file"], "sha256": f["sha256"],
                        "bytes": path.stat().st_size,
                        "source_date": build["source_timestamp"][:10]})
    return entries


def bundle(manifest, folder):
    builds = tomllib.loads(pathlib.Path(manifest).read_text()).get("builds", [])
    schema = schema_version()
    chosen, skipped = select_maps(builds, schema)
    for tag in skipped:
        print(f"::warning::skipping maps of build {tag}: not network schema {schema}; "
              "re-run the assets workflow")
    folder = pathlib.Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    downloaded = []
    for build, f in chosen:
        try:
            subprocess.run(["gh", "release", "download", build["tag"], "-R",
                            os.environ["GITHUB_REPOSITORY"], "-p", f["file"], "-D", str(folder)],
                           check=True)
            downloaded.append((build, f))
        except subprocess.CalledProcessError:
            # A deleted release shouldn't block the deploy; a bad hash still does.
            print(f"::warning::no map for {f['area']}: cannot download {f['file']} from {build['tag']}")
    entries = maps_manifest(downloaded, folder)
    (folder / "manifest.json").write_text(json.dumps(entries, indent=1) + "\n")
    print(f"maps: {', '.join(m['area'] for m in entries) or 'none'}")


if __name__ == "__main__":
    cmd, *args = sys.argv[1:] or ["help"]
    if cmd == "plan" and len(args) == 2:
        for name, bbox in plan(*args):
            print(name, bbox)
    elif cmd == "record" and len(args) >= 2:
        fields = dict(a.split("=", 1) for a in args[1:] if "=" in a)
        if "network_schema_version" in fields:
            fields["network_schema_version"] = int(fields["network_schema_version"])
        record(args[0], fields, [a for a in args[1:] if "=" not in a])
    elif cmd == "hash-rename" and args:
        for f in args:
            print(hash_rename(f))
    elif cmd == "schema" and not args:
        print(schema_version())
    elif cmd == "bundle" and len(args) == 2:
        bundle(*args)
    else:
        sys.exit(__doc__)
