#!/usr/bin/env python3
"""Set the `review-gate` commit status on a PR from its review comments.

Agents share one GitHub account, so reviews are PR comments (AGENTS.md §2.5):
a bold heading line `**Architect ...**` or `**UX ...**`, and a final
`VERDICT: APPROVE` / `VERDICT: REQUEST CHANGES` line. The latest such comment
per role decides that role's state.

Usage (in CI): GITHUB_TOKEN=... GITHUB_REPOSITORY=owner/repo review_gate.py <pr>
"""

import json
import os
import sys
import urllib.request

UX_PATHS = (
    "crates/rusthour-viz/",
    "crates/rusthour-cli/",
    "crates/rusthour-api/",
    "crates/rusthour-import/",
)
HEADINGS = {"**Architect": "architect", "**UX": "UX"}
TRUSTED = {"OWNER", "MEMBER", "COLLABORATOR"}


def verdict_of(body):
    """(role, verdict) for a review comment, or None if it isn't one."""
    lines = [line.strip() for line in body.splitlines()]
    first = next((line for line in lines if line), "")
    role = next((r for h, r in HEADINGS.items() if first.startswith(h)), None)
    verdict = None
    for line in lines:
        if line == "VERDICT: APPROVE":
            verdict = "approve"
        # A request may carry a trailing note; an approval must be exact.
        elif line.startswith("VERDICT: REQUEST CHANGES"):
            verdict = "request"
    return (role, verdict) if role and verdict else None


def is_release_pr(pr):
    """Opened by release-please (GITHUB_TOKEN) from this repo, not a fork."""
    head_repo = (pr["head"]["repo"] or {}).get("full_name")
    return (
        pr["user"]["login"] == "github-actions[bot]"
        and head_repo == pr["base"]["repo"]["full_name"]
        and pr["head"]["ref"].startswith("release-please--")
    )


def evaluate(pr, files, comments):
    """Return (state, description) for the commit status of PR info `pr`."""
    if is_release_pr(pr):
        return "success", "release PR: merged by the maintainer at milestone end"
    roles = ["architect"]
    if any(f.startswith(UX_PATHS) for f in files):
        roles.append("UX")
    state = {}
    for c in sorted(comments, key=lambda c: c["created_at"]):
        found = c["author_association"] in TRUSTED and verdict_of(c["body"] or "")
        if found:
            state[found[0]] = found[1]
    if all(state.get(r) == "approve" for r in roles):
        return "success", ", ".join(f"{r} ✓" for r in roles)
    problems = [f"{r} requested changes" for r in roles if state.get(r) == "request"]
    waiting = [f"{r} review" for r in roles if r not in state]
    if waiting:
        problems.append("waiting for: " + ", ".join(waiting))
    return "failure", "; ".join(problems)[:140]


def api(path, data=None):
    req = urllib.request.Request(
        f"https://api.github.com/repos/{os.environ['GITHUB_REPOSITORY']}/{path}",
        data=json.dumps(data).encode() if data else None,
        headers={
            "Authorization": f"Bearer {os.environ['GITHUB_TOKEN']}",
            "Accept": "application/vnd.github+json",
        },
    )
    with urllib.request.urlopen(req) as resp:
        return json.load(resp)


def paginate(path):
    items, page = [], 1
    while True:
        batch = api(f"{path}?per_page=100&page={page}")
        items += batch
        if len(batch) < 100:
            return items
        page += 1


def main(number):
    info = api(f"pulls/{number}")
    sha = info["head"]["sha"]

    def status(state, description):
        api(f"statuses/{sha}", {
            "state": state,
            "description": description,
            "context": "review-gate",
            "target_url": info["html_url"],
        })

    # Replace any older success first, so a failed run can't leave it green.
    status("pending", "evaluating review verdicts")
    try:
        files = [f["filename"] for f in paginate(f"pulls/{number}/files")]
        comments = paginate(f"issues/{number}/comments")
        state, description = evaluate(info, files, comments)
    except Exception:
        status("error", "review-gate run failed; see the workflow log")
        raise
    print(f"PR #{number} @ {sha}: {state}: {description}")
    status(state, description)


if __name__ == "__main__":
    main(int(sys.argv[1]))
