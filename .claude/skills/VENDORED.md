# Vendored skills

The `ponytail`, `ponytail-review` and `ponytail-debt` skills are copied
unchanged from [DietrichGebert/ponytail](https://github.com/DietrichGebert/ponytail)
(MIT, see [PONYTAIL-LICENSE](PONYTAIL-LICENSE)):

- Version: 5.3.2
- Commit: `09c434ab73ca43761c5d8b0c5d341926c2fba710` (2026-10-18)

They are committed here instead of installed as a plugin because cloud
sessions load a repository's `.claude/skills/` but not plugins enabled in its
`.claude/settings.json`. The plugin's hooks, statusline and other helper
skills are deliberately not included; agents load the skill explicitly
(AGENTS.md §1).

To update: copy `skills/<name>/SKILL.md` from a newer release, update the
version and commit above, and review the diff like any other change.
