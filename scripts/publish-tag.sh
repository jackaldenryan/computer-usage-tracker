#!/usr/bin/env bash
# Cut a release: verify versions agree, require a clean tree, tag v$VERSION,
# push the tag. GitHub Actions builds the release (see .github/workflows/release.yml).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

version="$(tr -d '[:space:]' < VERSION)"
pkg_version="$(node -p "require('./package.json').version")"
tauri_version="$(node -p "require('./src-tauri/tauri.conf.json').version")"
cargo_version="$(awk -F '"' '/^version = / { print $2; exit }' src-tauri/Cargo.toml)"

if [[ "$version" != "$pkg_version" || "$version" != "$tauri_version" || "$version" != "$cargo_version" ]]; then
  echo "Version mismatch: VERSION=$version package.json=$pkg_version tauri.conf.json=$tauri_version Cargo.toml=$cargo_version"
  exit 1
fi

tag="v$version"
if git rev-parse "$tag" >/dev/null 2>&1; then
  echo "Tag $tag already exists."
  exit 1
fi

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Working tree is dirty. Commit first."
  exit 1
fi

git tag "$tag"
git push origin "$tag"
echo "Pushed $tag. GitHub Actions will build the release."
