# Linux-Standalone Restructure — Design Spec

**Status:** Approved (brainstorming → plan)
**Date:** 2026-05-19
**Author:** Desmond Chen (with Claude)

## Problem

The repository was started as a personal fork of `yetone/voice-input-src`, a single-prompt-builds-a-macOS-app project. Over the past month the Linux side (Rust + GTK4 + wlr-layer-shell + whisper.cpp) has grown into a real shipping product (v0.1.0 / v0.1.1 with CPU and CUDA `.deb` packages on GitHub Releases), while the macOS side has not been touched. The repository's *self-description* — root `README.md`, `README_CN.md`, `dist/` submodule, `CLAUDE.md` — still talks exclusively about the macOS app and presents this repo as "a reproducibility wrapper around a Claude prompt." A new contributor cloning this repo today finds the README contradicts everything else.

## Goal

Restructure the repository so that its top-level identity matches reality: **a standalone Wayland-native voice-input project for Linux**, written in Rust, with `yetone/voice-input-src` credited as inspiration only.

Non-goals:

- Renaming the GitHub repo (`voice-input-src` stays — existing release URLs reference it).
- Rewriting `linux/README.md`: it is already accurate and complete. We only do surgical edits to absorb it as the new root README.
- Bumping action versions (Node 20 → 24 deprecation). Tracked separately.
- Any code change to the Rust crate.

## Decisions (approved during brainstorming)

| Decision | Choice |
|---|---|
| Directory layout | Flatten `linux/` into the repo root |
| `dist/` submodule (yetone macOS app) | Remove |
| `README.md` + `README_CN.md` (Claude prompt for macOS app) | Remove, replace root README with current `linux/README.md` |
| `LICENSE` copyright | Switch from `yetone, 2025` to `Desmond Chen, 2026` |
| Commit granularity | Multiple small commits (5), not one big commit |
| New macOS-app inspiration link | Single-paragraph `## Credits` section at the end of new README |

## File-level Changes

| Path | Action | Notes |
|---|---|---|
| `linux/Cargo.toml`, `linux/Cargo.lock` | `git mv` → repo root | |
| `linux/src/`, `linux/tests/`, `linux/scripts/` | `git mv` → repo root | |
| `linux/README.md` | `git mv` → repo root `README.md` (overwriting the macOS prompt) | Surgical edits applied in same commit (see *README edits* below) |
| `linux/assets/` if present | `git mv` → repo root | Verified at execution time |
| `README.md` (current — macOS prompt, English) | Overwritten by step above | |
| `README_CN.md` (macOS prompt, Chinese) | `git rm` | |
| `dist/` git submodule | `git rm dist`, delete `.git/modules/dist/` | |
| `.gitmodules` | `git rm .gitmodules` (it only contained `dist`) | |
| `CLAUDE.md` | Rewrite entirely (see *CLAUDE.md skeleton*) | |
| `LICENSE` | One-line edit: `Copyright (c) 2025 yetone` → `Copyright (c) 2026 Desmond Chen` | |
| `CHANGELOG.md` | No changes | Already Linux-only content |
| `plans/2026-05-15-voice-input-linux/`, `plans/2026-05-18-deb-release/` | No changes | Already at root |
| `input_test.txt` | `git rm --cached` | Already in `.gitignore`; should never have been tracked. Working-tree copy retained. |
| `.github/workflows/release.yml` | Edit 4 hunks (see *CI changes*) | |
| `.gitignore` | No structural change required | `target/` already matches a root-level `target/` after flatten |

## README edits (when `linux/README.md` becomes root `README.md`)

Six surgical edits to the file in the same commit it is moved:

1. Title `# VoiceInput (Linux)` → `# VoiceInput`
2. Remove the `cd linux` from the Build section (`cd linux && cargo build --release` → `cargo build --release`)
3. Path `./linux/scripts/install-autostart.sh` → `./scripts/install-autostart.sh`
4. Path `See ../plans/2026-05-15-voice-input-linux/...` → `See plans/2026-05-15-voice-input-linux/...`
5. Any other `linux/` path prefix in code-fence examples → strip the prefix
6. Append new `## Credits` section at the end:

   ```markdown
   ## Credits

   Inspired by [yetone/voice-input-src](https://github.com/yetone/voice-input-src),
   a Fn-key-driven voice input app for macOS. This project is a from-scratch
   Linux reimplementation in Rust targeting Wayland compositors — none of the
   original Swift code is included, but the UX shape (hold-to-talk capsule
   overlay + LLM refinement) traces back to that work.
   ```

The rest of the file (Install / Build / Run / Overlay / LLM Refinement / Compositor support / Config / Project layout / Troubleshooting) is preserved verbatim. No re-organization.

## CLAUDE.md skeleton (full rewrite)

```markdown
# CLAUDE.md

This file guides Claude Code working in this repository.

(one-paragraph intro: Wayland-native hold-to-talk voice input; single-target
 Rust crate; KDE Plasma 6 / sway / hyprland; explicitly not supported on GNOME)

## Build & Run

cargo build --release
cargo build --release --features cuda          # NVIDIA GPU users
cargo run --release                            # default: tray + listen + overlay
cargo run --release -- transcribe              # Phase 1 standalone mode
cargo test                                     # unit + integration tests

System deps (Debian/Ubuntu):
  cmake, clang, libclang-dev, libgtk-4-dev, libgtk4-layer-shell-dev,
  libwayland-dev, libxkbcommon-dev, libasound2-dev, pkg-config
GPU build adds: nvidia-cuda-toolkit

Runtime deps: ydotool (with running ydotool.service for paste), a Wayland
compositor supporting wlr-layer-shell, a whisper.cpp model under
~/.local/share/voice-input/models/

## Architecture

(ASCII flow diagram tailored to Linux:
  KeyMonitor(portal global-shortcuts) → AppOrchestrator → AudioCapture(cpal)
  → VadSlicer(Silero ONNX) → WhisperWorker(whisper.cpp) → LlmRefiner(optional)
  → TextInjector(ydotool); separate arrows for TrayMenu(ksni) and
  OverlayPanel(GTK4 + wlr-layer-shell))

Per-module file responsibilities — one line each, like the previous
CLAUDE.md had for the Swift sources but for src/{hotkey,audio,speech,
overlay,injector,tray,config,refiner}.rs etc.

## Conventions When Editing

- ydotool path is mandatory: wl-clipboard + Ctrl+V does not work under
  Wayland for cross-app paste; rewriting injector.rs must preserve the
  /dev/uinput dependency.
- Portal global-shortcuts identifies callers via cgroup scope (app-<id>-<PID>.scope).
  Do NOT package voice-input as a systemd --user service; portal refuses.
  XDG autostart is the supported launch path. See README troubleshooting.
- whisper.cpp builds via cmake; for CUDA, CMAKE_CUDA_ARCHITECTURES must be
  set explicitly in CI. Otherwise the native fallback hits sm_52 on a
  GPU-less runner and the .deb crashes on all modern cards.
- Persisted config lives in ~/.config/voice-input/config.toml — do not
  rename keys (whisper_model_path, llm_*, language_hint, etc.) without
  a migration.
- Tray icon uses freedesktop icon names ("microphone" idle,
  "media-record-symbolic" recording) — do not hardcode absolute paths.

## Release Process

1. Bump version in Cargo.toml on main.
2. Update CHANGELOG.md.
3. git tag vX.Y.Z && git push origin vX.Y.Z
4. GitHub Actions release.yml builds CPU + CUDA .deb in parallel,
   attaches both to a GitHub Release.
5. Verify both assets are present at
   https://github.com/desmondc9/voice-input-src/releases/tag/vX.Y.Z
```

The new CLAUDE.md must NOT carry over any of the macOS-specific guidance
(Cmd+V handling, TISSelectInputSource, NSPanel timing constants, Fn-key
suppression, LSUIElement) — those have no Linux analogue.

## CI changes (`.github/workflows/release.yml`)

Four hunks, all path corrections:

```diff
  - name: Build CPU .deb
-   working-directory: linux
    run: cargo deb

  - uses: actions/upload-artifact@v4
    with:
      name: deb-cpu
-     path: linux/target/debian/voice-input_*.deb
+     path: target/debian/voice-input_*.deb

  - name: Build CUDA .deb
-   working-directory: linux
    env:
      CMAKE_CUDA_ARCHITECTURES: "75;80;86;89;90"
    run: cargo deb --variant cuda

  - uses: actions/upload-artifact@v4
    with:
      name: deb-cuda
-     path: linux/target/debian/voice-input-cuda_*.deb
+     path: target/debian/voice-input-cuda_*.deb
```

The `release` job is unaffected (it consumes artifacts by name, not path).
`cargo deb` finds `./Cargo.toml` and `./target/debian/` automatically.

## Execution Order

Five commits, each independently buildable and shippable:

| # | Commit message | Verification before commit |
|---|---|---|
| 1 | `chore(repo): remove macOS-era artifacts (dist/, README_CN, original prompt)` | `git submodule status` empty; `ls dist/` not found; `cd linux && cargo build --release` still works (linux/ is untouched) |
| 2 | `refactor(repo): flatten linux/ to repo root` | `cargo build --release` at repo root succeeds; `cargo test` passes; no `linux/` directory remains |
| 3 | `ci: update release.yml paths after flatten` | YAML parses (`yq`); workflow re-tested only by next tag push — accepted risk |
| 4 | `docs(license): switch copyright to Desmond Chen` | `git diff LICENSE` is exactly one line |
| 5 | `docs: rewrite root README + CLAUDE.md for standalone project` | Markdown renders correctly in GitHub preview (manual); README links resolve |

Between commit 2 and commit 3 the release workflow is broken (it still
references `linux/...`). Tagging a release during that window would fail
the CI. Do not push tags until commit 3 lands.

## Out-of-Spec Follow-ups

These are intentionally NOT in this restructure:

- `git remote remove upstream` — local-only, no repo content
- Update GitHub repo description / topics on github.com — web action
- Bump `actions/checkout@v4` → `v5` and `actions/upload-artifact@v4` → `v5`
  for Node.js 24 — separate cleanup commit, ideally before the next tag
- Eventual GitHub repo rename to drop `-src` suffix — defer, breaks release URLs

## Verification of Done

After all 5 commits land on main:

1. `cargo build --release` from repo root succeeds
2. `cargo test` from repo root passes
3. Root `README.md` first heading reads `# VoiceInput` (no parenthetical)
4. Root `README.md` `## Credits` section exists and links to yetone
5. `dist/`, `README_CN.md`, `.gitmodules` are gone
6. `git log --oneline -5` shows the 5 commits in the listed order
7. Next `git tag vX.Y.Z && git push origin vX.Y.Z` produces a successful CI run with both .deb files attached
