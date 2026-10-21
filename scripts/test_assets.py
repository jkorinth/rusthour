"""Tests for assets.py. Run: python3 -I -m unittest discover -s scripts"""

import pathlib
import tempfile
import tomllib
import unittest

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
                "area": "bensheim-1km",
                "file": "bensheim-1km.osm.pbf",
                # printf pbf | sha256sum
                "sha256": "992d322a9bff6f443ca804811b135ed6c1b8f0064bdc8cff815714587c7a0184",
            }])


if __name__ == "__main__":
    unittest.main()
