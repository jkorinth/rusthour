"""Tests for pages.py. Run: python3 -m unittest discover -s scripts -p 'test_*.py'"""

import json
import unittest

from pages import release_assets, versions


def release(tag, *assets):
    return {"tag_name": tag, "assets": [{"name": a} for a in assets]}


class ReleaseAssets(unittest.TestCase):
    def test_skips_releases_without_the_web_build(self):
        releases = [
            release("v0.1.0"),
            release("v0.2.0", "rusthour-web-v0.2.0.zip", "other.zip"),
            release("v0.3.0", "rusthour-web-v0.2.0.zip"),  # wrong tag's zip
        ]
        self.assertEqual(release_assets(releases), ["v0.2.0"])

    def test_skips_tags_that_are_not_plain_versions(self):
        tags = ("../x", "nightly", "assets-2026-10-21", "v0.2.0\n", "v1.0.0")
        releases = [release(t, f"rusthour-web-{t}.zip") for t in tags]
        self.assertEqual(release_assets(releases), ["v1.0.0"])


class Versions(unittest.TestCase):
    def test_main_first_then_newest_release_by_semver(self):
        ids = [v["id"] for v in versions(["v0.9.0", "v0.10.0", "v0.2.1", "v1.0.0"])]
        self.assertEqual(ids, ["main", "v1.0.0", "v0.10.0", "v0.9.0", "v0.2.1"])

    def test_shape_is_json_list_of_id_and_label(self):
        doc = json.loads(json.dumps(versions(["v0.2.0"])))
        self.assertEqual(
            doc,
            [{"id": "main", "label": "main (development)"}, {"id": "v0.2.0", "label": "v0.2.0"}],
        )


if __name__ == "__main__":
    unittest.main()
