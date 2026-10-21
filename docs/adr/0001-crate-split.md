# ADR-0001: Crate split and dependency rules

- **Status:** accepted
- **Date:** 2026-10-21

## Context

Rust Hour has two very different faces: an interactive viewer (native and
browser) and a headless CLI for batch statistics. A remote-service mode is
planned. The simulation is performance-critical and must be reproducible.

## Decision

A Cargo workspace with four crates:

- `rusthour-core`: the simulation. No I/O, rendering, async, wall-clock time
  or global RNG. Fixed timestep with integer ticks; all randomness from one
  seeded `ChaCha8Rng`. Must build for `wasm32-unknown-unknown`.
- `rusthour-api`: the only entry point to the core. Serde-serializable,
  versioned protocol types (`PROTOCOL_VERSION`) and a `Simulation` facade
  trait, implemented in-process now and remotely later. The `server` feature
  is native-only.
- `rusthour-viz` and `rusthour-cli`: frontends that depend on `rusthour-api`
  only.

The dependency direction `core` ← `api` ← {`viz`, `cli`} is enforced by
`cargo-deny` (a `bans` entry allowing only `rusthour-api` to wrap
`rusthour-core`).

Determinism is guarded by a stable FNV-1a state hash (`StateHasher`) and a
golden-hash test. `std`'s `DefaultHasher` is avoided because its output may
change between Rust releases.

## Consequences

- Anything a frontend can do, a remote client can do too.
- Frontends cannot reach into core internals for convenience; rendering needs
  efficient snapshot types in `rusthour-api` (designed in M1/M2).
- The core stays portable and testable without a window or network.
- Every new piece of engine state must be fed into `Engine::state_hash`, or
  divergence goes unnoticed.
