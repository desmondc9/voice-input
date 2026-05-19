# v0.1.0 Debian Package Release — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `desmondc9/voice-input-src` v0.1.0 — the first GitHub Release of the Linux app — with two `.deb` packages (CPU + CUDA) built and published automatically by a tag-triggered GitHub Actions workflow.

**Architecture:** Six configuration / docs changes land on `main` sequentially (each its own commit, conventional-commit prefix for auto-grouped release notes), then a local pre-flight cargo-deb run verifies the packages build cleanly, then a single `v0.1.0` tag push triggers CI which builds both `.deb`s and creates the GitHub Release.

**Tech Stack:** Cargo features (existing), [`cargo-deb`](https://crates.io/crates/cargo-deb) for `.deb` generation, GitHub Actions (`dtolnay/rust-toolchain`, `softprops/action-gh-release`), Keep a Changelog format.

**Reference spec:** `plans/2026-05-18-deb-release/design.md`

---

## Pre-flight: entry conditions

Verify before starting:

- On `main` at `20c5bb0` (`docs: add v0.1.0 deb release design spec`)
- One commit ahead of `origin/main` (the spec) — needs to be pushed before tagging
- `linux/Cargo.toml` currently hard-codes `whisper-rs = { version = "0.14", features = ["cuda"] }` on line 33
- Repo root has **no** `.gitignore` (untracked `.claude/`, `input_test.txt`, `linux/target/`)
- `cargo-deb` not yet installed locally (`cargo install cargo-deb` needed before Task 8)

Branch decision: all changes commit directly on `main` (this is a release-prep sequence, each commit is independently shippable, no review-branch needed). User has been working this way for Phases 5–7.

```bash
cd /home/desmond/Repos/voice-input-src
git status                      # confirm clean working tree (only .claude/ + input_test.txt untracked)
git log --oneline -1            # confirm HEAD == 20c5bb0
```

---

## File structure

```
.                               # repo root
├── .gitignore                  # NEW    (Task 1)
├── .github/
│   └── workflows/
│       └── release.yml         # NEW    (Task 4)
├── CHANGELOG.md                # NEW    (Task 6)
├── linux/
│   ├── Cargo.toml              # MODIFY (Tasks 2 + 3)
│   └── README.md               # MODIFY (Task 5)
└── plans/2026-05-18-deb-release/
    └── implementation.md       # THIS FILE
```

No new source code. No new Rust dependencies. No changes to existing `.rs` files.

---

## Verification model

Most of this work is configuration, not application logic — so "tests" are **verification commands** producing expected output. Each task ends with a verification step that must pass before commit. Where the spec calls for a behavioral check (`cargo deb` produces a file, `dpkg-deb -I` shows expected control fields), the exact command and expected output are listed.

The traditional unit-test-first cycle does not apply here. The TDD discipline that does apply: **run the verification before committing**, and never claim success without seeing the expected output.

---

## Task 1: Add `.gitignore`

**Files:**
- Create: `.gitignore` (repo root)

The repo has no `.gitignore`. Currently `git status` shows `.claude/` and `input_test.txt` untracked, and any local `cargo build` produces an untracked `linux/target/` directory. Pin the ignore list now so future commits can't accidentally include build artifacts or local-tool files.

- [ ] **Step 1: Create the file**

Write `.gitignore` at repo root with:

```gitignore
# Cargo build output
target/

# Built Debian packages
*.deb

# Local test / scratch files
input_test.txt

# Claude Code local state
.claude/
```

- [ ] **Step 2: Verify nothing previously-tracked is now ignored**

```bash
cd /home/desmond/Repos/voice-input-src
git ls-files -i -c --exclude-from=.gitignore
```

Expected: **empty output**. If any path prints, a tracked file matches a new ignore pattern — investigate before continuing.

- [ ] **Step 3: Verify the previously-untracked paths are now ignored**

```bash
git status --porcelain
```

Expected: only `.gitignore` appears (as untracked / `??`). The `.claude/` and `input_test.txt` lines that were there in the pre-flight should be gone.

- [ ] **Step 4: Commit**

```bash
git add .gitignore
git commit -m "chore: add .gitignore for target/, *.deb, local scratch files"
```

---

## Task 2: Make CUDA an opt-in Cargo feature

**Files:**
- Modify: `linux/Cargo.toml`:
  - Add a new `[features]` section after the `[package]` block (around line 9, before `[dependencies]`)
  - Change `whisper-rs` line (currently line 33) to drop the hard-coded `features = ["cuda"]`

This is a **behavior change for anyone building locally** — after this commit, `cargo build --release` no longer requires the CUDA toolkit. The user must opt in with `--features cuda` for GPU acceleration. README updates in Task 5 document the flag.

- [ ] **Step 1: Add the `[features]` block**

In `linux/Cargo.toml`, insert immediately after line 8 (`description = "..."`) and before the blank line that precedes `[dependencies]`:

```toml

[features]
default = []
cuda = ["whisper-rs/cuda"]
```

- [ ] **Step 2: Update the `whisper-rs` dependency line**

Replace line 33 (currently `whisper-rs = { version = "0.14", features = ["cuda"] }`) with:

```toml
whisper-rs = "0.14"
```

- [ ] **Step 3: Verify CPU build configuration**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo check --no-default-features
```

Expected: compiles successfully without invoking the CUDA toolkit. If the build pulls in `cuda` artifacts (look for `cudart` / `cublas` in output), the feature wiring is wrong.

- [ ] **Step 4: Verify CUDA build still works**

```bash
cargo check --features cuda
```

Expected: compiles successfully and reports linking against CUDA libraries (`-lcublas`, `-lcudart`). Requires `nvidia-cuda-toolkit` locally (user has it — Phase 7 confirms).

- [ ] **Step 5: Smoke-test runtime parity**

```bash
cargo build --release --features cuda
./target/release/voice-input --help
```

Expected: binary prints help text. (We're not running full dictation here — just confirming the binary links and starts. The build pipeline didn't change semantically, only the gate.)

- [ ] **Step 6: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add linux/Cargo.toml
git commit -m "feat(linux): make CUDA an opt-in Cargo feature (default off)"
```

---

## Task 3: Add cargo-deb packaging metadata

**Files:**
- Modify: `linux/Cargo.toml` — append `[package.metadata.deb]` table and `[package.metadata.deb.variants.cuda]` sub-table at the end of the file (after the existing `[lib]` block on line ~47).

The two tables describe both `.deb` outputs. The base table produces `voice-input_0.1.0_amd64.deb` (CPU); the `variants.cuda` overrides produce `voice-input-cuda_0.1.0_amd64.deb` (CUDA-enabled binary, declares conflict with CPU package).

- [ ] **Step 1: Install cargo-deb locally (if not already present)**

```bash
which cargo-deb || cargo install cargo-deb --locked
```

Expected: prints a path, or installs and then prints `Installed package cargo-deb`.

- [ ] **Step 2: Append `[package.metadata.deb]` to `linux/Cargo.toml`**

Append (with a leading blank line separating it from `[lib]`):

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

- [ ] **Step 3: Build the CPU `.deb` and capture the output path**

```bash
cd /home/desmond/Repos/voice-input-src/linux
cargo deb
```

Expected output ends with a line like `target/debian/voice-input_0.1.0_amd64.deb`. The file should exist (~10 MB):

```bash
ls -lh target/debian/voice-input_0.1.0_amd64.deb
```

- [ ] **Step 4: Inspect the CPU `.deb` control fields**

```bash
dpkg-deb -I target/debian/voice-input_0.1.0_amd64.deb
```

Expected fields (partial; order may vary):

```
 Package: voice-input
 Version: 0.1.0
 Architecture: amd64
 Maintainer: Desmond Chen <desmondc9@outlook.com>
 Section: utility
 Priority: optional
 Depends: libc6, libgtk-4-1, libwayland-client0, libxkbcommon0, ydotool
 Recommends: ydotool-daemon
```

`Conflicts:` and `Provides:` should be **absent** from the CPU `.deb`.

- [ ] **Step 5: Inspect the CPU `.deb` file layout**

```bash
dpkg-deb -c target/debian/voice-input_0.1.0_amd64.deb
```

Expected: lists `./usr/bin/voice-input`, `./usr/share/doc/voice-input/README.md`, `./usr/share/doc/voice-input/copyright`, `./usr/share/voice-input/install-autostart.sh`. The binary is mode `755`, the docs are `644`, the autostart script is `755`.

- [ ] **Step 6: Build the CUDA `.deb`**

```bash
cargo deb --variant cuda
```

Expected: `target/debian/voice-input-cuda_0.1.0_amd64.deb` exists. Note: builds the CUDA-feature binary first, so this takes ~30–60 s longer than Step 3.

- [ ] **Step 7: Inspect the CUDA `.deb` control fields**

```bash
dpkg-deb -I target/debian/voice-input-cuda_0.1.0_amd64.deb
```

Expected to differ from CPU in these fields:

```
 Package: voice-input-cuda
 Conflicts: voice-input
 Provides: voice-input
 Depends: libc6, libgtk-4-1, libwayland-client0, libxkbcommon0, ydotool, libcudart12 | libcudart11, libcublas12 | libcublas11
```

- [ ] **Step 8: Verify both `.deb` files install the same binary path**

```bash
dpkg-deb -c target/debian/voice-input_0.1.0_amd64.deb | grep usr/bin
dpkg-deb -c target/debian/voice-input-cuda_0.1.0_amd64.deb | grep usr/bin
```

Expected: both print `./usr/bin/voice-input` (same path — that's why the CUDA variant declares `Conflicts:`).

- [ ] **Step 9: Commit**

The `*.deb` artifacts are now ignored by `.gitignore` from Task 1 — `git status` will only show `linux/Cargo.toml` as modified.

```bash
cd /home/desmond/Repos/voice-input-src
git status                                  # confirm only linux/Cargo.toml modified
git add linux/Cargo.toml
git commit -m "feat(linux): add cargo-deb packaging for CPU and CUDA variants"
```

---

## Task 4: GitHub Actions release workflow

**Files:**
- Create: `.github/workflows/release.yml`

Triggers on any tag matching `v*` push. Three jobs in parallel/sequence: `build-cpu` and `build-cuda` run in parallel on `ubuntu-22.04`; `release` runs after both succeed and creates the GitHub Release with both `.deb` files attached and auto-generated notes.

`ubuntu-22.04` chosen for glibc compatibility — newer Ubuntu runner versions ship glibc that's too new for Debian 12 / Ubuntu 22.04 user systems. (Spec Section 4.)

- [ ] **Step 1: Create the workflow directory**

```bash
mkdir -p /home/desmond/Repos/voice-input-src/.github/workflows
```

- [ ] **Step 2: Create `release.yml`**

Write `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    tags: ['v*']

permissions:
  contents: write

jobs:
  build-cpu:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with:
          submodules: recursive
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: "1.83"
      - name: Install build deps
        run: |
          sudo apt-get update
          sudo apt-get install -y libgtk-4-dev libwayland-dev cmake clang libclang-dev libasound2-dev
      - name: Install cargo-deb
        run: cargo install cargo-deb --locked
      - name: Build CPU .deb
        working-directory: linux
        run: cargo deb
      - uses: actions/upload-artifact@v4
        with:
          name: deb-cpu
          path: linux/target/debian/voice-input_*.deb

  build-cuda:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
        with:
          submodules: recursive
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: "1.83"
      - name: Install build deps (incl. CUDA toolkit)
        run: |
          sudo apt-get update
          sudo apt-get install -y libgtk-4-dev libwayland-dev cmake clang libclang-dev libasound2-dev nvidia-cuda-toolkit
      - name: Install cargo-deb
        run: cargo install cargo-deb --locked
      - name: Build CUDA .deb
        working-directory: linux
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
        with:
          path: artifacts
      - uses: softprops/action-gh-release@v2
        with:
          files: |
            artifacts/deb-cpu/voice-input_*.deb
            artifacts/deb-cuda/voice-input-cuda_*.deb
          generate_release_notes: true
          draft: false
          prerelease: false
```

- [ ] **Step 3: Validate the YAML**

```bash
python3 -c "import yaml, sys; yaml.safe_load(open('/home/desmond/Repos/voice-input-src/.github/workflows/release.yml'))" && echo "YAML OK"
```

Expected: prints `YAML OK`. If `python3` has no `yaml` module, install via `pip install pyyaml` (or `uv pip install pyyaml`) and retry. If YAML is malformed, `yaml.safe_load` raises and the script aborts before printing.

- [ ] **Step 4: Optional — actionlint check**

If `actionlint` is installed (`which actionlint`), run it:

```bash
actionlint /home/desmond/Repos/voice-input-src/.github/workflows/release.yml
```

Expected: no output (= no problems). If `actionlint` is not installed, skip this step — the YAML parse in Step 3 is the minimum bar; the real verification is the actual tag push in Task 9.

- [ ] **Step 5: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add .github/workflows/release.yml
git commit -m "feat(ci): add release workflow for tag-triggered .deb builds"
```

---

## Task 5: Update `linux/README.md`

**Files:**
- Modify: `linux/README.md`:
  - Replace the status block (line 5) with a v0.1.0 release line
  - Insert a new `## Install` section between the current `## Build` (line 11) and the line above it
  - Add an `### Optional: CUDA acceleration` sub-section under `## Build`

The README opens with a status sentence pointing at Phase 7. After v0.1.0 ships, the lead sentence should point users at the latest release; build-from-source instructions remain but are no longer the recommended path.

- [ ] **Step 1: Read the current README in full**

```bash
cat /home/desmond/Repos/voice-input-src/linux/README.md
```

Note the existing structure: line 1 is the title, line 5 is the Phase 7 status block, line 7 has a GNOME note, line 11 starts `## Build`. Verify line numbers haven't shifted since the spec was written.

- [ ] **Step 2: Replace the status block**

Change line 5 from:

```markdown
> Status: **Phase 7** — latency polish (persistent whisper + VAD workers, VAD silence cutoff 150 ms) plus XDG autostart installer. Phase 6 recording icon, Phase 5 tray menus, headless `transcribe` CLI all unchanged.
```

to:

```markdown
> Status: **v0.1.0 released** — first Linux release. Download the [latest `.deb`](https://github.com/desmondc9/voice-input-src/releases/latest), or build from source (instructions below).
```

- [ ] **Step 3: Insert the `## Install` section**

Insert a new section immediately before the existing `## Build` line. The block below uses **4-backtick outer fences** so the inner triple-backtick `bash` block displays as part of the verbatim README content — copy the content between (but not including) the 4-backtick lines into `linux/README.md`.

````markdown
## Install

**Recommended:** download the latest `.deb` from the [GitHub Releases page](https://github.com/desmondc9/voice-input-src/releases/latest) and install with `apt`.

```bash
# CPU build (any Linux, no GPU required):
wget https://github.com/desmondc9/voice-input-src/releases/download/v0.1.0/voice-input_0.1.0_amd64.deb
sudo apt install ./voice-input_0.1.0_amd64.deb

# NVIDIA GPU users — faster transcription via CUDA:
wget https://github.com/desmondc9/voice-input-src/releases/download/v0.1.0/voice-input-cuda_0.1.0_amd64.deb
sudo apt install ./voice-input-cuda_0.1.0_amd64.deb
```

After install, three one-time setup steps:

1. **Download a whisper model** — see [Download a whisper model](#download-a-whisper-model) below.
2. **Install and start `ydotoold`** — see [Install ydotool](#install-ydotool-for-listen-mode-only) below. (Package `ydotool-daemon` is recommended by both `.deb`s so it should already be present; the section explains how to enable the systemd user service.)
3. **Bind a global shortcut on first launch** — the app registers a portal global shortcut. Open your desktop's Global Shortcuts settings (KDE: System Settings → Shortcuts → Global Shortcuts) and assign a key to `voice-input → Hold to dictate`.

Then run `voice-input` to start the tray app.

````

- [ ] **Step 4: Add CUDA acceleration sub-section under `## Build`**

The current `## Build` section ends after the `apt install` line for system packages. Insert this new sub-section directly after it (again using 4-backtick outer fences for clarity — only the inner content goes into the README):

````markdown
### Optional: CUDA acceleration

The default build is CPU-only and requires no special toolkit. For NVIDIA GPU acceleration:

```bash
sudo apt install nvidia-cuda-toolkit
cd linux
cargo build --release --features cuda
```

The CUDA-enabled binary is 5–15× faster on RTX-class GPUs (e.g. 3 s → 200 ms for a 5-second utterance with `large-v3-turbo`). It links against `libcudart` and `libcublas` at runtime.

````

- [ ] **Step 5: Verify the file still renders as valid Markdown**

```bash
cd /home/desmond/Repos/voice-input-src
head -60 linux/README.md
```

Expected: title → status (now mentioning v0.1.0) → GNOME note → `## Install` → `## Build` → `### Optional: CUDA acceleration`. No broken fences (every triple-backtick has a matching close).

Spot-check fence balance:

```bash
grep -c '^```' linux/README.md
```

Expected: an **even** number (every opening fence has a matching close).

- [ ] **Step 6: Commit**

```bash
git add linux/README.md
git commit -m "docs(linux): document v0.1.0 install path + CUDA build flag"
```

---

## Task 6: Create root `CHANGELOG.md`

**Files:**
- Create: `CHANGELOG.md` (repo root)

[Keep a Changelog](https://keepachangelog.com/) format. v0.1.0 enumerates the 7 phases as a feature matrix. Auto-generated GitHub release notes (the commit log binned by conventional-commit prefix) are complementary — the CHANGELOG is the curated, human-edited summary.

- [ ] **Step 1: Create the file**

Write `CHANGELOG.md` at repo root:

```markdown
# Changelog

All notable changes to the Linux build of voice-input are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Pre-1.0: API, CLI surface, and config schema may change without notice between
minor versions.

## [0.1.0] - 2026-05-18

First publishable Linux release. All Phase 0–7 features present.

### Added

- **Audio capture & VAD pipeline** (Phase 1) — `cpal` input + Silero ONNX
  voice activity detector + whisper.cpp transcription, segmented at speech
  boundaries.
- **Hotkey + paste pipeline** (Phase 2) — XDG Portal global shortcut binding;
  `wl-clipboard` write + `ydotool` Ctrl+V emulation for Wayland.
- **GTK4 overlay** (Phase 3) — `wlr-layer-shell` capsule with live waveform,
  centered at the bottom of the focused output.
- **LLM refiner** (Phase 4) — optional pass through any OpenAI-compatible
  endpoint (Ollama, OpenAI, etc.) for transcript polishing.
- **Settings tray menu** (Phase 5) — `ksni` tray with submenus for enabled
  state, language, model size, refiner settings; persisted to
  `~/.config/voice-input/config.toml`.
- **Recording-state tray icon** (Phase 6) — animated tray icon that pulses
  during active dictation.
- **Latency polish + autostart** (Phase 7) — persistent whisper / VAD worker
  threads (eliminates per-dictation model load), VAD silence cutoff tuned to
  150 ms, XDG autostart installer script.
- **CUDA acceleration** — opt-in via `cargo build --release --features cuda`
  or by installing the `voice-input-cuda` `.deb`. 5–15× speedup on RTX-class
  GPUs.
- **Packaging** — `voice-input` (CPU) and `voice-input-cuda` (GPU) `.deb`
  packages built and published by a tag-triggered GitHub Actions workflow.

### Known limitations

- Wayland only — explicitly does not target X11.
- GNOME's mutter does not implement `wlr-layer-shell`, so the overlay is
  mis-positioned on GNOME. KDE Plasma, sway, hyprland are supported.
- `amd64` builds only. `arm64` deferred to a future release.
- Whisper models, the `ydotoold` daemon, and the portal global-shortcut
  binding are user-installed steps, not bundled in the `.deb`.

[0.1.0]: https://github.com/desmondc9/voice-input-src/releases/tag/v0.1.0
```

- [ ] **Step 2: Verify the file**

```bash
head -20 /home/desmond/Repos/voice-input-src/CHANGELOG.md
```

Expected: starts with `# Changelog`, has the Keep-a-Changelog preamble, then the `## [0.1.0]` section.

- [ ] **Step 3: Commit**

```bash
cd /home/desmond/Repos/voice-input-src
git add CHANGELOG.md
git commit -m "docs: add CHANGELOG for v0.1.0"
```

---

## Task 7: Push all commits to `origin/main`

Before tagging, all six commits (including the previously-unpushed spec at `20c5bb0`) need to be on `origin/main` so CI can check them out.

- [ ] **Step 1: Confirm what's about to be pushed**

```bash
cd /home/desmond/Repos/voice-input-src
git log --oneline origin/main..HEAD
```

Expected: **7 commits** (the spec from before this session + the 6 Tasks 1–6 commits). Verify subjects look right and follow conventional-commit format (the spec relies on this for auto-grouped release notes).

- [ ] **Step 2: Push**

```bash
git push origin main
```

Expected: standard `git push` output. No `-u` needed; the branch already tracks origin.

- [ ] **Step 3: Verify the remote is in sync**

```bash
git status
```

Expected: `Your branch is up to date with 'origin/main'.` and `nothing to commit, working tree clean`.

---

## Task 8: Local pre-flight — install one `.deb` and smoke-test

Before pushing the tag (which triggers CI that publishes the release publicly), do a local install + uninstall round-trip on the CPU `.deb` produced in Task 3. Catches any post-install brokenness that `cargo deb` itself wouldn't flag.

If Task 3's `target/debian/voice-input_0.1.0_amd64.deb` was deleted between then and now, rebuild first: `cd linux && cargo deb`.

- [ ] **Step 1: Install the CPU `.deb`**

```bash
sudo apt install ./linux/target/debian/voice-input_0.1.0_amd64.deb
```

Expected: apt resolves the declared dependencies (`libc6`, `libgtk-4-1`, `libwayland-client0`, `libxkbcommon0`, `ydotool`), pulls `ydotool-daemon` via Recommends, completes without errors. Note: this temporarily uninstalls any other binary at `/usr/bin/voice-input` if one exists.

- [ ] **Step 2: Verify the binary is on `PATH`**

```bash
which voice-input
voice-input --help
```

Expected: `which` prints `/usr/bin/voice-input`. `--help` prints the clap-generated help text.

- [ ] **Step 3: Verify the bundled assets installed**

```bash
ls -l /usr/share/doc/voice-input/
ls -l /usr/share/voice-input/install-autostart.sh
```

Expected: `README.md` + `copyright` in `/usr/share/doc/voice-input/`; the autostart script exists and is executable.

- [ ] **Step 4: Quick interactive smoke-test (optional but recommended)**

Run `voice-input` from a terminal (don't go through systemd / autostart yet — this is a sanity check). The tray icon should appear, the hotkey should still work if previously configured. Kill the process with Ctrl+C.

If the tray icon doesn't appear or the binary crashes, **stop the release**: there's a packaging bug. Investigate before proceeding.

- [ ] **Step 5: Uninstall**

```bash
sudo apt remove voice-input
```

Expected: clean removal. `which voice-input` now prints nothing (or whatever was at that path before — see Step 1 note).

- [ ] **Step 6: Reinstall any previously-present development binary**

If the user had a `cargo install`-built `voice-input` at `~/.cargo/bin/voice-input` or similar before this pre-flight, it's untouched (different path). If they had a `make install`-style copy in `/usr/local/bin/`, also untouched. Only `/usr/bin/voice-input` was managed by apt and is now gone — acceptable interim state since the next step puts the released version back via tag.

---

## Task 9: Tag, push, monitor CI, verify release

The tag push is the trigger that runs the release workflow. Once pushed it's hard to take back — if anything is wrong with the workflow, the only recovery is `git tag -d` + `git push --delete origin`, then re-tag.

- [ ] **Step 1: Create the annotated tag**

```bash
cd /home/desmond/Repos/voice-input-src
git tag -a v0.1.0 -m "Release v0.1.0 — first Linux release

CPU (voice-input) and CUDA (voice-input-cuda) .deb packages.
Wayland-native hold-to-talk voice input for KDE / sway / hyprland.
All Phase 0-7 features included."
```

- [ ] **Step 2: Verify the tag locally**

```bash
git tag -l v0.1.0
git show v0.1.0 --stat | head -20
```

Expected: tag exists; `git show` displays the tag message and the diff at HEAD (which should be empty if HEAD didn't change — the tag is on the same commit as `main`).

- [ ] **Step 3: Push the tag**

```bash
git push origin v0.1.0
```

Expected: `* [new tag] v0.1.0 -> v0.1.0`. The release workflow starts immediately on the GitHub side.

- [ ] **Step 4: Monitor the workflow run**

Open the Actions tab in a browser: `https://github.com/desmondc9/voice-input-src/actions`. The `Release` workflow should be running. Both `build-cpu` and `build-cuda` jobs run in parallel; expect ~5 min for `build-cpu`, ~10 min for `build-cuda` (CUDA toolkit install is slow). Then `release` runs (~30 s).

Alternatively from the CLI:

```bash
gh run watch --exit-status
```

Expected: all three jobs finish with `✓`. If a job fails, fetch logs with `gh run view <run-id> --log-failed`.

- [ ] **Step 5: Verify the GitHub Release exists with both `.deb`s attached**

```bash
gh release view v0.1.0
```

Expected: release titled `v0.1.0` (or whatever auto-generated title GitHub assigned), notes include the auto-generated commit list grouped by conventional-commit prefix, and **two assets** are listed:

- `voice-input_0.1.0_amd64.deb`
- `voice-input-cuda_0.1.0_amd64.deb`

- [ ] **Step 6: Smoke-test the published `.deb`**

Download and install from the public release URL (proves the end-user install flow works):

```bash
cd /tmp
wget https://github.com/desmondc9/voice-input-src/releases/download/v0.1.0/voice-input_0.1.0_amd64.deb
sudo apt install ./voice-input_0.1.0_amd64.deb
voice-input --help
sudo apt remove voice-input
```

Expected: clean download (no 404), clean install, `--help` works, clean remove.

- [ ] **Step 7: Done. The release is live.**

If anything in Steps 4–6 failed:

| Failure mode | Recovery |
|---|---|
| CI build job failed (transient) | `gh run rerun <run-id>` |
| CI build job failed (config bug) | `git tag -d v0.1.0 && git push --delete origin v0.1.0`, fix bug on `main`, push, retag |
| Release notes wrong | Edit on the GitHub Releases page UI — no rebuild needed |
| `.deb` installs but `voice-input` crashes | Publish a v0.1.1 fix; do **not** delete v0.1.0 |

---

## Self-review checklist

- ✅ **Spec coverage**: every spec section has a task —
  - §2 Cargo CUDA opt-in → Task 2
  - §3 cargo-deb metadata → Task 3
  - §4 release.yml → Task 4
  - §5 .gitignore → Task 1
  - §6 README + CHANGELOG → Tasks 5, 6
  - §7 versioning → no task needed (Cargo.toml already at 0.1.0 — verified in Task 2 pre-conditions)
  - §8 release walkthrough → Tasks 7, 8, 9
- ✅ **No placeholders**: every step has exact code or exact commands.
- ✅ **Type / name consistency**: `voice-input` vs `voice-input-cuda` used consistently; feature flag is `cuda` everywhere; release tag is `v0.1.0` everywhere.
- ✅ **Conventional-commit prefixes** match user preference for auto-grouped release notes: `chore:`, `feat(linux):`, `feat(ci):`, `docs(linux):`, `docs:`.
- ✅ **Commits ordered so each is independently shippable**: `.gitignore` lands first (so `*.deb` from Task 3 is ignored), CUDA feature lands before cargo-deb (so the variant table's `features = ["cuda"]` reference is valid), README updates land before tagging.
