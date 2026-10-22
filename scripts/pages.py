"""Assemble the GitHub Pages site: every release's web build plus the version frame.

Usage: python3 scripts/pages.py <site-dir>
Expects <site-dir>/main/ to hold the current build. Needs GH_TOKEN and
GITHUB_REPOSITORY; downloads each release's rusthour-web-<tag>.zip into
<site-dir>/<tag>/, then writes versions.json and the frame index.html.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

# Plain release tags only: they become directory names and URL paths.
TAG = re.compile(r"v(\d+)\.(\d+)\.(\d+)")


def release_assets(releases):
    """Tags of releases that carry their own rusthour-web-<tag>.zip."""
    return [
        r["tag_name"]
        for r in releases
        if TAG.fullmatch(r["tag_name"])
        and f"rusthour-web-{r['tag_name']}.zip" in {a["name"] for a in r["assets"]}
    ]


def versions(tags):
    """versions.json: main first, then releases newest first."""
    ordered = sorted(tags, key=lambda t: tuple(map(int, TAG.fullmatch(t).groups())), reverse=True)
    return [{"id": "main", "label": "main (development)"}] + [{"id": t, "label": t} for t in ordered]


def main(site):
    repo = os.environ["GITHUB_REPOSITORY"]
    pages = subprocess.run(
        ["gh", "api", "--paginate", "--slurp", f"repos/{repo}/releases"],
        check=True, capture_output=True, text=True,
    ).stdout
    tags = release_assets(r for page in json.loads(pages) for r in page if not r["draft"])
    with tempfile.TemporaryDirectory() as tmp:
        for tag in tags:
            name = f"rusthour-web-{tag}.zip"
            subprocess.run(["gh", "release", "download", tag, "-R", repo, "-p", name, "-D", tmp], check=True)
            with zipfile.ZipFile(Path(tmp, name)) as z:
                z.extractall(site / tag)
            print(f"added {tag}")
    (site / "versions.json").write_text(json.dumps(versions(tags), indent=1) + "\n")
    shutil.copy(Path(__file__).parent.parent / "pages" / "index.html", site / "index.html")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
