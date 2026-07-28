#!/usr/bin/env bash
#
# Build the Arch package from the current HEAD, exactly the way
# .github/workflows/publish-arch-repo.yml does, without publishing anything.
# Use it to rehearse PKGBUILD changes before pushing them.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
packaging_dir="$repo_root/packaging"

pkgver="$(node -p "require('$repo_root/src-tauri/tauri.conf.json').version")"
pkgrel="${PKGREL:-1}"

# git archive packages the committed tree, so an uncommitted change silently
# building into the package is not possible.
if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=normal)" ]]; then
  echo "note: working tree is dirty; packaging HEAD, not your local changes" >&2
fi

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

git -C "$repo_root" archive --format=tar.gz --prefix=src/ \
  -o "$workdir/cosmic-mail-$pkgver.tar.gz" HEAD

cp "$packaging_dir"/{PKGBUILD,cosmic-mail.desktop,cosmic-mail.service,cosmic-mail.install} \
  "$workdir/"

cd "$workdir"
# -d skips dependency resolution: node usually comes from a version manager
# rather than pacman here, and -s would try to install a system nodejs to
# satisfy makedepends. CI builds in a clean container and is authoritative on
# whether the declared dependencies are actually complete.
PKGVER="$pkgver" PKGREL="$pkgrel" makepkg -f -d --noconfirm

built="$(ls "$workdir"/*.pkg.tar.zst)"
mv "$built" "$repo_root/"
echo
echo "Built: $repo_root/$(basename "$built")"
echo "Install it with: sudo pacman -U $repo_root/$(basename "$built")"
