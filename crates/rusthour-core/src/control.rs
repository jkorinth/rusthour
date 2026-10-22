//! Intersection controllers: priority, stop, signals, roundabouts (M2, M3,
//! M7). Each lives in its own submodule implementing a common trait and is
//! dispatched statically through one enum (ADR-0004).
