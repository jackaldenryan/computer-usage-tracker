#!/bin/bash
# Stage the headless helper binary so the app bundle ships it as a resource.
# Runs automatically as part of `tauri build` (beforeBuildCommand).
# Cwd when invoked by Tauri is the project root.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$ROOT/src-tauri/resources/bin"
# Placeholder first: the Tauri build script validates that bundled resources
# exist when compiling, so on a fresh checkout this satisfies it until the
# real binary is copied over below. (The app ignores 0-byte files.)
touch "$ROOT/src-tauri/resources/bin/wht-track"
cargo build --release --bin wht-track --manifest-path "$ROOT/src-tauri/Cargo.toml"
cp "$ROOT/src-tauri/target/release/wht-track" "$ROOT/src-tauri/resources/bin/wht-track"
echo "Staged helper: $ROOT/src-tauri/resources/bin/wht-track"
