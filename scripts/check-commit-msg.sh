#!/usr/bin/env sh
# Rejects commit messages that are not Conventional Commits (AGENTS.md §3).
#
# Usage: check-commit-msg.sh [--strict] <commit-msg-file>
#
# Default (local commit-msg hook): `fixup!`/`squash!`/`amend!` and local merge
# commits are allowed, so review fixes can be folded in later with
# `git rebase -i --autosquash`.
# --strict (CI, on every commit of a PR): they are rejected, because PRs are
# rebase-merged and every commit lands on main as-is.
set -eu

strict=0
if [ "${1:-}" = "--strict" ]; then
  strict=1
  shift
fi

subject=$(head -n 1 "$1")

case "$subject" in
  fixup!\ * | squash!\ * | amend!\ * | Merge\ *)
    if [ "$strict" -eq 1 ]; then
      echo "error: this commit would land on main as-is: $subject" >&2
      echo "clean up history first: git rebase -i --autosquash origin/main" >&2
      exit 1
    fi
    exit 0
    ;;
esac

pattern='^(feat|fix|perf|refactor|test|docs|build|ci|chore|revert)(\([a-z0-9._-]+\))?!?: [^ ].*$'

if ! printf '%s\n' "$subject" | grep -Eq "$pattern"; then
  echo "error: commit subject is not a Conventional Commit:" >&2
  echo "  $subject" >&2
  echo "expected: <type>(<scope>): <summary>, e.g. 'feat(core): add IDM car-following'" >&2
  echo "types: feat fix perf refactor test docs build ci chore revert" >&2
  exit 1
fi
