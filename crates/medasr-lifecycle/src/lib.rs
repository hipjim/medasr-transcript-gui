//! Orchestrator: owns state, dispatches to peers. Implementation lands
//! across Units 5, 9C and the rest of the lifecycle work.
//!
//! This is the only library crate in the workspace allowed >3 peer
//! `medasr-*` deps; binaries (`medasr-cli`, `src-tauri`) are exempt from
//! the same rule. See `scripts/check-dep-dag.sh`.
#![forbid(unsafe_code)]
