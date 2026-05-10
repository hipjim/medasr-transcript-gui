# Release runbook

End-to-end procedure for cutting a MedASR release. Builds run on GitHub
Actions; nothing has to compile on your local machine.

## One-time setup

1. Push this repo to GitHub:

   ```bash
   gh repo create elostar/med-asr --public --source=. --remote=origin --push
   ```

   Replace `--public` with `--private` if you don't want it public.

2. Confirm the workflows arrived: open `Actions` in the repo on GitHub.

## Cutting a release

```bash
# from a clean main / feature branch
git tag v0.1.0
git push origin v0.1.0
```

Pushing the tag triggers `.github/workflows/release.yml`, which:

1. Builds `medasr-gui` in **release mode** on three runners in parallel:
   - `macos-14`        → `medasr-vX.Y.Z-macos-arm64.zip`
   - `windows-latest`  → `medasr-vX.Y.Z-windows-x64.zip`
   - `ubuntu-22.04`    → `medasr-vX.Y.Z-linux-x64.tar.gz`
2. Each archive contains: `medasr-gui[.exe]` + `LICENSE` + `README.md` +
   `FIRST-RUN.txt` (a short setup note for end users).
3. After all three builds succeed, a fourth job creates a GitHub Release
   tagged `vX.Y.Z` and attaches the three archives.

The first build on a fresh runner is **slow** (sherpa-onnx-sys compiles
its bundled C++ library statically — 5–15 min on Windows, 3–8 min on
macOS / Linux). Subsequent builds reuse `Swatinem/rust-cache@v2`, which
caches both the cargo registry and the `target/` dir keyed on the
toolchain target; rebuilds against the same target finish in ~1 min.

## Dry-running the workflow

Open the **Actions** tab on GitHub, pick the *Release* workflow, click
`Run workflow`. The build job runs on the matrix but the publish step is
skipped (only `tags/v*` triggers it). Use this to verify a green
cross-platform build before tagging.

## What does NOT ship in the release archive

- **The MedASR model weights** (`model.int8.onnx`, ~150 MB). They are
  governed by Google's HAI-DEF Terms of Use, which we honour by
  downloading the model on first launch with an explicit EULA-acceptance
  step (see `crates/medasr-model/`). Bundling them in the release would
  short-circuit that gate and is not currently authorised.
- **The Tauri shell** (`src-tauri/`). The shipping app is the egui-based
  `medasr-gui`. The Tauri scaffold remains in the workspace as a future
  alternative front-end but is not bundled.

## What you DO need to ship

- Code-sign and notarize the macOS `.app` for distribution outside the
  repo (Apple Developer ID + `notarytool`). Currently the macOS zip
  contains a bare unsigned binary; Gatekeeper will require the user to
  right-click → Open the first time. Notarisation is **not** wired into
  the release workflow yet — see Phase 3 / Unit 10 in the plan.
- Code-sign the Windows `.exe` with an EV cert (or accept a SmartScreen
  warning on first download). Also not wired in yet.

These belong in a follow-up `release.yml` revision once the signing
identities are procured.

## Versioning

Bump the version once, in the workspace root `Cargo.toml` under
`workspace.package.version`. All crates inherit it via
`version.workspace = true`.

The release archive name embeds the git tag (e.g. `v0.1.0`) — if you
forget to bump `Cargo.toml` first, the `--version` output of the binary
will not match the release tag. Future improvement: a tiny CI step that
fails the workflow when `Cargo.toml` and the tag disagree.

## Rollback

A bad release is just a bad GitHub Release. Delete it from the **Releases**
page (the underlying tag stays). Push a new tag with a higher patch
version once fixed; CI builds and publishes.
