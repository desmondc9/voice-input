# v0.1.0 Debian Package Release — Design Spec

> **Status:** approved 2026-05-18. Next step: implementation plan via `superpowers:writing-plans`.

## 1. Goal & Non-Goals

**Goal:** Publish the Linux app at `desmondc9/voice-input-src` as a first-ever GitHub Release **v0.1.0**, including two `.deb` packages — `voice-input` (CPU) and `voice-input-cuda` (NVIDIA GPU accelerated). Release is fully driven by a GitHub Actions workflow that triggers on the `v0.1.0` tag push: it builds both packages and creates the release with auto-generated notes.

**Non-goals:**
- macOS release (handled upstream in `dist/` submodule by yetone).
- Bundling whisper models, Ollama, or ydotool inside the `.deb` — users install those separately via `README.md` instructions.
- arm64 builds. v0.1.0 ships `amd64` only.
- Non-Wayland (X11) support — explicitly out of scope.

## 2. Cargo.toml — Make CUDA an Opt-In Feature

The current `linux/Cargo.toml` hard-codes `whisper-rs = { version = "0.14", features = ["cuda"] }`, which forces every build to require the CUDA toolkit and locks out non-NVIDIA users. Restructure so CUDA is an optional Cargo feature, default off.

```toml
[features]
default = []
cuda = ["whisper-rs/cuda"]

[dependencies]
whisper-rs = "0.14"  # default features only — no cuda
# ... other deps unchanged ...
```

Build commands after this change:

| Use | Command | Requires CUDA toolkit? |
|---|---|---|
| Default (CPU) | `cargo build --release` | No |
| CUDA accelerated | `cargo build --release --features cuda` | Yes (`nvidia-cuda-toolkit`) |

This is a behavior change for anyone currently building locally. `linux/README.md` must document the new flag (Section 6).

## 3. cargo-deb Metadata & Two-Variant Packaging

Use the **`cargo-deb`** crate to generate `.deb` files (`cargo install cargo-deb`). Configure via `[package.metadata.deb]` in `linux/Cargo.toml`. Two variants — the base table for the CPU package, an `variants.cuda` table for the GPU package.

```toml
[package.metadata.deb]
maintainer = "Desmond Chen <desmondc9@outlook.com>"
copyright = "2026, Desmond Chen <desmondc9@outlook.com>"
license-file = ["../LICENSE", "0"]
extended-description = """\
Wayland-native hold-to-talk voice input for Linux. Hold the configured
hotkey, speak, release — the transcript is pasted into the focused app.
Tray menu for runtime config; optional LLM refinement via any
OpenAI-compatible endpoint."""
section = "utility"
priority = "optional"
depends = "libc6, libgtk-4-1, libwayland-client0, libxkbcommon0, ydotool"
recommends = "ydotool-daemon"
assets = [
    ["target/release/voice-input", "usr/bin/", "755"],
    ["README.md", "usr/share/doc/voice-input/README.md", "644"],
    ["../LICENSE", "usr/share/doc/voice-input/copyright", "644"],
    ["scripts/install-autostart.sh", "usr/share/voice-input/install-autostart.sh", "755"],
]

[package.metadata.deb.variants.cuda]
name = "voice-input-cuda"
conflicts = "voice-input"
provides = "voice-input"
depends = "libc6, libgtk-4-1, libwayland-client0, libxkbcommon0, ydotool, libcudart12 | libcudart11, libcublas12 | libcublas11"
features = ["cuda"]
```

**Outputs:**

| File | Approx size | Audience |
|---|---|---|
| `voice-input_0.1.0_amd64.deb` | ~10 MB | Any Linux user |
| `voice-input-cuda_0.1.0_amd64.deb` | ~10 MB (links libcudart/libcublas) | NVIDIA GPU + CUDA driver/toolkit users |

**Conflicts/provides rationale:** both `.deb` files install `/usr/bin/voice-input`. The CUDA package declares `Conflicts: voice-input` so the user can't install both simultaneously, and `Provides: voice-input` so any future package that depends on `voice-input` is satisfied by either variant.

**`Recommends: ydotool-daemon`** rather than `Depends:` — Ubuntu/Debian ship `ydotool` (the CLI) and `ydotool-daemon` (the daemon) as separate packages. Some users may already have the daemon running via a different setup. Recommends installs it by default but doesn't block.

## 4. GitHub Actions Workflow

Create `.github/workflows/release.yml`. Triggers on `v*` tag push. Three jobs:

```yaml
name: Release

on:
  push:
    tags: ['v*']

permissions:
  contents: write   # gh release create

jobs:
  build-cpu:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with: { submodules: recursive }
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: "1.83" }
      - run: sudo apt-get update && sudo apt-get install -y libgtk-4-dev libwayland-dev cmake clang libclang-dev libasound2-dev
      - run: cargo install cargo-deb --locked
      - working-directory: linux
        run: cargo deb
      - uses: actions/upload-artifact@v4
        with:
          name: deb-cpu
          path: linux/target/debian/voice-input_*.deb

  build-cuda:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with: { submodules: recursive }
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: "1.83" }
      - run: sudo apt-get update && sudo apt-get install -y libgtk-4-dev libwayland-dev cmake clang libclang-dev libasound2-dev nvidia-cuda-toolkit
      - run: cargo install cargo-deb --locked
      - working-directory: linux
        run: cargo deb --variant cuda
      - uses: actions/upload-artifact@v4
        with:
          name: deb-cuda
          path: linux/target/debian/voice-input-cuda_*.deb

  release:
    needs: [build-cpu, build-cuda]
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/download-artifact@v4
        with: { path: artifacts }
      - uses: softprops/action-gh-release@v2
        with:
          files: |
            artifacts/deb-cpu/voice-input_*.deb
            artifacts/deb-cuda/voice-input-cuda_*.deb
          generate_release_notes: true
          draft: false
          prerelease: false
```

**Notes:**

1. **`generate_release_notes: true`** — uses GitHub's API to bin commits since the last tag (or all commits, for v0.1.0) by conventional-commit prefix. Our `feat:` / `fix:` / `perf:` / `docs:` discipline groups cleanly.

2. **CUDA toolkit install in CI is slow (~5 min)** — accepted for v0.1.0. If maintenance burden grows, add `actions/cache` for `/usr/lib/cuda` etc. as a v0.2.0 improvement.

3. **`submodules: recursive`** — the repo has a `dist/` submodule (macOS sources). Recursive checkout ensures CI has full context, though cargo-deb only needs files inside `linux/` plus the root `LICENSE` (referenced via `../LICENSE`).

4. **Failure recovery** — if any build job fails, `release` job won't run (gated by `needs:`). The tag remains. Recovery: delete the tag (`git tag -d v0.1.0 && git push --delete origin v0.1.0`), fix the bug, retag, repush. Tags cannot be reused without delete-republish, so this is the only path.

5. **CPU job intentionally omits `nvidia-cuda-toolkit`** — proves CPU build does not require it, matching the documented user contract.

## 5. .gitignore

Currently the repo has no `.gitignore`. Create one at the root:

```
target/
*.deb
input_test.txt
.claude/
```

Already untracked but worth pinning so future commits don't accidentally include them.

## 6. README & CHANGELOG Updates

### `linux/README.md`

1. **Status block (line 5)** — replace Phase-status sentence with the v0.1.0 release line. Link to `https://github.com/desmondc9/voice-input-src/releases/latest`.

2. **New `## Install` section** — placed BEFORE `## Build`. Contents:
   - "Recommended: install from the latest `.deb` release."
   - Two-line apt-style command:
     ```bash
     # CPU build (any Linux):
     wget https://github.com/desmondc9/voice-input-src/releases/download/v0.1.0/voice-input_0.1.0_amd64.deb
     sudo apt install ./voice-input_0.1.0_amd64.deb

     # NVIDIA GPU users (faster):
     wget https://github.com/desmondc9/voice-input-src/releases/download/v0.1.0/voice-input-cuda_0.1.0_amd64.deb
     sudo apt install ./voice-input-cuda_0.1.0_amd64.deb
     ```
   - Post-install setup (3 ordered steps): download a whisper model, install/start `ydotoold`, bind a portal global shortcut on first launch. Each step links to its existing README sub-section.

3. **`## Build` updated** — add a sub-section "Optional CUDA acceleration":
   - "Default build is CPU-only and requires no special toolkit."
   - CUDA prerequisites + `cargo build --release --features cuda`.

### Root `CHANGELOG.md` (new file)

Format: [Keep a Changelog](https://keepachangelog.com/). v0.1.0 section enumerates the seven phases as a feature matrix (one or two bullets per phase). Subsequent releases append above v0.1.0.

GitHub's auto-generated release notes (commit list) are still useful — they're consumed by readers of the Release page. The CHANGELOG is for readers of the repo who want a curated, human-edited summary.

### Root `README.md` (unchanged)

The top-level `README.md` and `README_CN.md` describe the upstream macOS spec. They are not touched.

## 7. Versioning

- Cargo.toml `version` stays at `0.1.0` (already set). No bump needed for v0.1.0 release.
- Future bumps follow semver: bug fixes → `0.1.x`, additive features → `0.x.0`, breaking changes → `x.0.0`.
- v0.1.0 = "first publishable cut, all Phase 0-7 features present". Pre-1.0 means API/CLI/config schema may shift in 0.2.0+ — set user expectations in CHANGELOG.

## 8. Release Walkthrough (v0.1.0)

```bash
# (a) All design changes — Cargo.toml feature flag, [package.metadata.deb] tables,
# .github/workflows/release.yml, .gitignore, linux/README.md, root CHANGELOG.md —
# land on main via the implementation plan's PR/commits.

# (b) Local pre-flight: run cargo-deb both modes, install one, smoke test, uninstall.
cd linux
cargo install cargo-deb
cargo deb                       # → target/debian/voice-input_0.1.0_amd64.deb
cargo deb --variant cuda        # → voice-input-cuda_0.1.0_amd64.deb (needs nvidia-cuda-toolkit locally)
sudo apt install ./target/debian/voice-input_0.1.0_amd64.deb
voice-input                     # confirm tray appears + hotkey works
sudo apt remove voice-input

# (c) Tag and push. CI takes over.
git tag -a v0.1.0 -m "Release v0.1.0 — first Linux release"
git push origin v0.1.0

# (d) Visit GitHub Releases page. Auto-generated notes will be present.
# Edit if minor polish desired; both .deb attached.
```

**Risks & recovery:**

- *CI build fails.* Delete tag (`git tag -d v0.1.0 && git push --delete origin v0.1.0`), fix bug, retag.
- *.deb installs but voice-input crashes on launch.* Publish v0.1.1 with the fix; don't delete v0.1.0 (users with it can compare).
- *cargo-deb mis-bundles assets.* Caught by the local pre-flight in step (b). The pre-flight is non-optional.

## 9. Open Questions (none)

All decisions resolved during brainstorming. Implementation plan can proceed.

---

**Spec self-review** (run at write time):

- ✅ No TBD/TODO placeholders.
- ✅ Internal consistency: cargo-deb invocation paths (`linux/`) match GH Actions `working-directory`. `license-file = ["../LICENSE", "0"]` matches `assets` block's `../LICENSE` path.
- ✅ Scope: single project (the Linux .deb release). No subsystem decomposition needed.
- ✅ Ambiguity: `Recommends: ydotool-daemon` chosen over `Depends:` deliberately (Section 3); `default = []` is the explicit choice for the Cargo features block (Section 2). Both pinned.
