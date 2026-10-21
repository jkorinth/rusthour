# Rust Hour — Agent Instructions

Read this file and [ROADMAP.md](ROADMAP.md) before doing any work. The roadmap
says **what** to build; this file says **how**.

## 1. Roles

Every piece of work involves three agent roles. One agent may not review its
own work.

| Role | Responsibilities |
| --- | --- |
| **Software architect** | Owns crate boundaries, public APIs, data layout, performance strategy. Writes ADRs in `docs/adr/`. Reviews **every** PR for design, boundaries and roadmap fit. |
| **Software engineer** | Implements roadmap items test-first. **Always loads and follows the `ponytail` skill** at the start of every engineering task. Authors PRs. |
| **UX designer** | Owns the interactive experience (`rusthour-viz`), CLI ergonomics, output formats, error messages and API ergonomics. Reviews every PR that touches these. |

Additionally, at the end of each milestone, **user-trial agents** use the
software as a real user would (§8).

## 2. Workflow

### During M0 (scaffolding)

Direct commits to `main` are allowed. Conventional commits are still mandatory.

### From the M0 release onward

1. **Claim** a roadmap item: set its status to `claimed` and owner to your
   branch name. Edit only your own item's line.
2. **Branch** per item: `<type>/<ID>-<slug>`, e.g. `feat/M3.1-mobil`.
   Use a separate git worktree per item so agents can work in parallel.
3. **Test first** (§5), then implement.
4. **Open a PR** using the template. Title is a conventional commit
   (it becomes the squash commit). Reference the roadmap ID.
5. **Review**: architect always; UX designer if the PR touches `rusthour-viz`,
   `rusthour-cli` UX, output formats or `rusthour-api` public types. Reviewers post a
   comment ending with an explicit verdict: `VERDICT: APPROVE` or
   `VERDICT: REQUEST CHANGES` (agents may share a GitHub account, so formal
   approvals may not be available).
6. **Merge** by squash once CI is green and required verdicts are APPROVE.
   Set the roadmap item to `done` in the same PR.

PRs stay small: one roadmap item, or part of one. Never mix unrelated changes.

## 3. Conventional commits (mandatory)

Format: `<type>(<scope>): <summary>` — types `feat`, `fix`, `perf`,
`refactor`, `test`, `docs`, `build`, `ci`, `chore`. Scope is the crate
(`core`, `api`, `viz`, `cli`) or area (`ci`, `deps`, `roadmap`). Breaking
changes use `!` and a `BREAKING CHANGE:` footer.

Enforced by a commit-msg hook (lefthook) and a PR-title check in CI.
release-please derives versions and changelogs from these commits; the release
PR is merged only at milestone completion. Use a `Release-As:` footer if the
computed version doesn't match the milestone.

## 4. Linting (continuous)

Runs locally via lefthook and on every push/PR in CI. Warnings are errors.

| Tool | Purpose |
| --- | --- |
| `rustfmt` | Formatting (config committed) |
| `clippy` | `-D warnings`; workspace lints in root `Cargo.toml` with `clippy::pedantic` enabled and documented exceptions |
| `cargo-deny` | Licenses, advisories, banned/duplicate crates, sources |
| `cargo-shear` | Unused dependencies |
| `typos` | Spelling in code and docs |
| `taplo` | TOML formatting |
| `markdownlint-cli2` | Markdown |
| `actionlint`, `zizmor` | GitHub Actions correctness and security |

`unsafe_code` is forbidden by default. Exceptions need an ADR or architect
approval, a module-level `allow`, and `// SAFETY:` comments.

## 5. Testing policy

- **Test-driven:** write the failing test first, then the code.
- **Don't overconstrain:** a few well-defined tests per feature (typically
  2–5) that pin down the roadmap item's acceptance criteria — not every line.
- **Regressions:** every bug fix adds exactly one test reproducing it, named
  `regression_<issue-number>_<slug>`.
- **Determinism:** any change to `rusthour-core` must keep the determinism test
  passing (same seed ⇒ same state hash). Intentional changes update the golden
  hash in the same PR with a note.
- Run tests with `cargo nextest run`. Benchmarks (criterion) are tracked but
  not gating until the architect says otherwise.

## 6. Architecture rules

- **Dependency direction:** `rusthour-core` ← `rusthour-api` ← {`rusthour-viz`, `rusthour-cli`}.
  Frontends never depend on `rusthour-core` directly. CI checks this.
- **`rusthour-core`:** no I/O, no rendering, no async, no wall-clock time, no global
  RNG. Must compile to `wasm32-unknown-unknown`. Fixed timestep; all
  randomness from the scenario seed.
- **Performance:** structure-of-arrays for vehicle state, per-lane sorted
  vectors, no per-vehicle hashmaps or allocations in the step loop.
- **`rusthour-api`:** serde-serializable, versioned protocol types; one
  `Simulation` facade with in-process and (later) remote implementations. The
  `server` feature is native-only.
- **Extension points over shared files:** new intersection controllers, driver
  profiles, metrics etc. go in their own module behind a trait, so parallel
  PRs rarely touch the same file.

## 7. Parallel work

- Respect module ownership in `rusthour-core` (`network`, `routing`, `vehicle`,
  `behavior`, `control`, `demand`, `scenario`, `stats`). If your item needs a
  change in another module's public interface, open a small separate PR for
  that first and get architect approval.
- Rebase on `main` before requesting review. Resolve `Cargo.lock` conflicts by
  regenerating, not hand-editing.
- If two items conflict, the architect decides ordering and marks one
  `blocked` in the roadmap.

## 8. Milestone user trials

Before the release PR for a milestone is merged:

1. One or two agents act as users with no prior context: fresh clone, follow
   the README only.
2. They exercise the milestone's features the way a person would — run the CLI
   scenarios, open the browser build and use the interactive controls (via a
   browser tool if available, otherwise headless screenshots).
3. They write `docs/trials/M<N>.md`: what they tried, what worked, what was
   confusing, screenshots where useful.
4. Bugs become issues (each fix gets a regression test); UX issues go to the UX
   designer for triage. Blocking findings must be fixed before release.
