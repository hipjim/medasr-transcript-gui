# MedASR

A local-only push-to-talk dictation desktop app for radiologists, built on
Google's MedASR Conformer-CTC model. Audio and PHI never leave the device.

> **Status: Unit 1 scaffold.** The plan ([`docs/plans/2026-05-07-001-feat-medasr-desktop-dictation-plan.md`](docs/plans/2026-05-07-001-feat-medasr-desktop-dictation-plan.md))
> defines 11 implementation units across 3 phases. This commit lays out the
> Cargo workspace, CI, and the foundational primitives. No dictation works yet.

## Architecture (so far)

Cargo workspace with 16 crates plus a Tauri 2 host:

```
medasr-types          (leaf — FocusTarget, AudioBuffer, errors)
medasr-paths          (per-OS app-data, model cache, audit, settings paths)
medasr-secure-buffer  (mlock'd, zero-on-drop PHI buffer)
medasr-postprocess    (voice-command/number/caps transforms — Unit 8)
medasr-state          (push-to-talk state machine — Unit 5; headless)
medasr-audio          (cpal/rubato/VAD — Unit 2)
medasr-asr            (sherpa-onnx wrapper for MedASR-CTC — Unit 3)
medasr-inject         (KeystrokeBackend trait + impls — Units 4.5, 6)
medasr-focus          (two-factor focus pin — Unit 4.5)
medasr-permissions    (mic / accessibility / input-monitoring — Unit 7)
medasr-hotkey         (global push-to-talk hotkey — Unit 4.5)
medasr-model          (model fetch + verify + TLS pin + EULA — Unit 4)
medasr-audit          (local diagnostic event log — Unit 9A)
medasr-settings       (schema-versioned settings — Unit 9B)
medasr-lifecycle      (orchestrator)
medasr-cli            (Phase 1 milestone CLI)
src-tauri             (Tauri 2 host + tray)
```

Dependency rules (enforced by [`scripts/check-dep-dag.sh`](scripts/check-dep-dag.sh)):

1. `medasr-types` is a leaf.
2. `medasr-state` never depends on platform crates.
3. Library crates have at most 3 `medasr-*` peer deps (`medasr-lifecycle`
   excepted; binaries `medasr-cli` and `src-tauri` exempt).
4. Only `medasr-model` may depend on HTTP-client crates
   (`reqwest` / `hyper` / `ureq` / `http` / `isahc`).

## Building (Unit 1)

Requires Rust 1.80+, Node 18+ (for the Vite UI), platform GUI deps for Tauri.

```bash
# Workspace check
cargo check --workspace

# Run the dep-dag enforcer
bash scripts/check-dep-dag.sh

# Run the secure-buffer tests
cargo test -p medasr-secure-buffer
```

The scaffold compiles and passes its tests; it does nothing useful yet.

## Privacy posture (current)

- v1 is a **personal/research artifact**, not a hospital-IT-deployed product.
- No telemetry, no auto-update plugin compiled in.
- `SecureBuffer` mlocks PHI buffers (best-effort — limited by
  `RLIMIT_MEMLOCK`; no defense against full-RAM hibernation).
- Network egress is restricted via the Tauri capability layer plus
  SPKI-pinned TLS for model downloads (no OS-level Network Extension /
  WFP / nftables filter compiled in).
- Audit log is a plain rotating event log — no HMAC chain in v1.

These choices reflect the scope refinement during the Phase 1 review.

## License

Code: Apache-2.0 (see [`LICENSE`](LICENSE)).

The MedASR model weights downloaded by this app are governed by Google's
HAI-DEF Terms of Use, accepted at first run. They are not redistributed by
this repository.
