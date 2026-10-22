"""Tests for assets.py. Run: python3 -I -m unittest discover -s scripts"""

import contextlib
import hashlib
import io
import json
import pathlib
import shutil
import subprocess
import tempfile
import tomllib
import unittest
from unittest import mock

import assets

AREAS = pathlib.Path(__file__).resolve().parent.parent / "data" / "areas.toml"


class PlanTest(unittest.TestCase):
    def test_bbox_is_reordered_for_osmium(self):
        # areas.toml is [south, west, north, east]; osmium wants W,S,E,N.
        self.assertEqual(assets.osmium_bbox([49.6765, 8.6161, 49.6855, 8.6299]),
                         "8.6161,49.6765,8.6299,49.6855")

    def test_empty_input_means_all_areas(self):
        names = [name for name, _ in assets.plan(AREAS, " ")]
        self.assertEqual(names, ["bensheim-1km", "bensheim-2km", "bensheim-5km"])

    def test_selected_areas_are_trimmed_and_deduplicated(self):
        plan = assets.plan(AREAS, "bensheim-5km, bensheim-1km,bensheim-5km")
        self.assertEqual([name for name, _ in plan], ["bensheim-5km", "bensheim-1km"])

    def test_bad_names_are_rejected(self):
        for bad in ["nowhere", "Bensheim-1km", "../x", "a;rm -rf", "bensheim-1km,,"]:
            with self.subTest(bad=bad), self.assertRaises(SystemExit):
                assets.plan(AREAS, bad)


class RecordTest(unittest.TestCase):
    def test_builds_are_appended_as_valid_toml(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            manifest = tmp / "assets.toml"
            manifest.write_text("# header comment\n")
            extract = tmp / "bensheim-1km.osm.pbf"
            extract.write_bytes(b"pbf")
            for tag in ["assets-2026-10-21", "assets-2026-11-01"]:
                assets.record(manifest, {"tag": tag, "tool": 'osmium "1.16"'}, [extract])

            data = tomllib.loads(manifest.read_text())
            self.assertTrue(manifest.read_text().startswith("# header comment\n"))
            self.assertEqual([b["tag"] for b in data["builds"]],
                             ["assets-2026-10-21", "assets-2026-11-01"])
            self.assertEqual(data["builds"][0]["tool"], 'osmium "1.16"')
            self.assertEqual(data["builds"][0]["files"], [{
                "kind": "osm.pbf",
                "area": "bensheim-1km",
                "file": "bensheim-1km.osm.pbf",
                # printf pbf | sha256sum
                "sha256": "992d322a9bff6f443ca804811b135ed6c1b8f0064bdc8cff815714587c7a0184",
            }])

    def test_rhnet_files_get_their_kind_and_area(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            manifest = tmp / "assets.toml"
            extract = tmp / "bensheim-1km.osm.pbf"
            extract.write_bytes(b"pbf")
            rhnet = tmp / "bensheim-1km.rhnet"
            rhnet.write_bytes(b"map")
            rhnet = assets.hash_rename(rhnet)
            assets.record(manifest, {"tag": "assets-2026-10-21", "network_schema_version": 2},
                          [extract, rhnet])

            build = tomllib.loads(manifest.read_text())["builds"][0]
            sha = hashlib.sha256(b"map").hexdigest()
            self.assertEqual(rhnet.name, f"bensheim-1km.{sha}.rhnet")
            self.assertEqual(build["network_schema_version"], 2)
            self.assertEqual([(f["kind"], f["area"]) for f in build["files"]],
                             [("osm.pbf", "bensheim-1km"), ("rhnet", "bensheim-1km")])
            self.assertEqual(build["files"][1]["file"], rhnet.name)


def rhnet(area, data=b"map"):
    sha = hashlib.sha256(data).hexdigest()
    return {"kind": "rhnet", "area": area, "file": f"{area}.{sha}.rhnet", "sha256": sha}


def build(tag, schema, timestamp, *files):
    return {"tag": tag, "network_schema_version": schema, "source_timestamp": timestamp,
            "files": [{"kind": "osm.pbf", "area": "a", "file": "a.osm.pbf", "sha256": "0"},
                      *files]}


class MapsTest(unittest.TestCase):
    def test_schema_version_is_read_from_the_checkout(self):
        self.assertIsInstance(assets.schema_version(), int)

    def test_each_area_gets_its_newest_build_of_the_current_schema(self):
        full = build("assets-1", 2, "2026-10-01T00:00:00Z", rhnet("a", b"1"), rhnet("b", b"1"))
        partial = build("assets-2", 2, "2026-10-05T00:00:00Z", rhnet("a", b"2"))
        same_time = build("assets-4", 2, "2026-10-05T00:00:00Z", rhnet("a", b"4"))
        bump = build("assets-3", 3, "2026-10-09T00:00:00Z", rhnet("b", b"3"))
        # Merge order isn't build order; equal timestamps: the later entry wins.
        chosen, skipped = assets.select_maps([partial, bump, full, same_time], 2)
        self.assertEqual([(b["tag"], f["area"]) for b, f in chosen],
                         [("assets-4", "a"), ("assets-1", "b")])
        self.assertEqual(skipped, ["assets-3"])
        self.assertEqual(assets.select_maps([full], 3), ([], ["assets-1"]))

    def test_bad_names_and_tags_are_rejected_before_any_download(self):
        good = rhnet("bensheim-5km")
        for field, value in [("file", "*"), ("file", f"../{good['file']}"),
                             ("file", f"other.{good['sha256']}.rhnet"), ("area", "../x"),
                             ("sha256", "ABC"), ("tag", "--repo=evil")]:
            entry = dict(good, **{field: value} if field != "tag" else {})
            b = build(value if field == "tag" else "assets-1", 2, "2026-10-21", entry)
            with self.subTest(field=field, value=value), self.assertRaises(SystemExit):
                assets.select_maps([b], 2)
        with tempfile.TemporaryDirectory() as tmp:
            manifest = pathlib.Path(tmp, "assets.toml")
            manifest.write_text('[[builds]]\ntag = "assets-1"\nnetwork_schema_version = '
                                f'{assets.schema_version()}\nsource_timestamp = "2026-10-21"\n'
                                '[[builds.files]]\nkind = "rhnet"\narea = "a"\nfile = "*"\n'
                                f'sha256 = "{"0" * 64}"\n')
            with mock.patch("subprocess.run") as run, self.assertRaises(SystemExit):
                assets.bundle(manifest, pathlib.Path(tmp, "maps"))
            run.assert_not_called()

    def test_manifest_json_follows_the_viewer_contract(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            self.assertEqual(assets.maps_manifest([], tmp), [])
            entry = rhnet("bensheim-5km")
            (tmp / entry["file"]).write_bytes(b"map")
            b = build("assets-1", 2, "2026-10-21T20:21:52Z", entry)
            self.assertEqual(assets.maps_manifest([(b, entry)], tmp), [{
                "area": "bensheim-5km", "file": entry["file"], "sha256": entry["sha256"],
                "bytes": 3, "source_date": "2026-10-21",
            }])
            (tmp / entry["file"]).write_bytes(b"evil")
            with self.assertRaises(SystemExit):
                assets.maps_manifest([(b, entry)], tmp)

    def test_a_missing_release_drops_only_its_maps(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            manifest, src, out = tmp / "assets.toml", tmp / "src", tmp / "maps"
            src.mkdir()
            for tag, area in [("assets-2026-10-01", "a"), ("assets-2026-10-02", "b")]:
                (src / f"{area}.rhnet").write_bytes(area.encode())
                assets.record(manifest, {"tag": tag, "source_timestamp": tag[7:],
                                         "network_schema_version": assets.schema_version()},
                              [assets.hash_rename(src / f"{area}.rhnet")])

            def download(cmd, check):
                tag, pattern, folder = cmd[3], cmd[cmd.index("-p") + 1], cmd[-1]
                if tag == "assets-2026-10-01":
                    raise subprocess.CalledProcessError(1, cmd)
                shutil.copy(src / pattern, folder)

            with mock.patch("subprocess.run", side_effect=download), \
                    mock.patch.dict("os.environ", GITHUB_REPOSITORY="x/y"), \
                    contextlib.redirect_stdout(io.StringIO()) as log:
                assets.bundle(manifest, out)
            maps = json.loads((out / "manifest.json").read_text())
            self.assertEqual([m["area"] for m in maps], ["b"])
            self.assertIn("::warning::no map for a", log.getvalue())


if __name__ == "__main__":
    unittest.main()
