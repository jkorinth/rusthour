#!/usr/bin/env sh
# Rejects commit messages that are not Conventional Commits (AGENTS.md §3).
# Usage: check-commit-msg.sh <commit-msg-file>
set -eu

subject=$(head -n 1 "$1")

case "$subject" in
  Merge\ *|Revert\ *|fixup!\ *|squash!\ *) exit 0 ;;
esac

pattern='^(feat|fix|perf|refactor|test|docs|build|ci|chore|revert)(\([a-z0-9._-]+\))?!?: [^ ].*$'

if ! printf '%s\n' "$subject" | grep -Eq "$pattern"; then
  echo "error: commit subject is not a Conventional Commit:" >&2
  echo "  $subject" >&2
  echo "expected: <type>(<scope>): <summary>, e.g. 'feat(core): add IDM car-following'" >&2
  echo "types: feat fix perf refactor test docs build ci chore revert" >&2
  exit 1
fi
