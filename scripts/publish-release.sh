#!/usr/bin/env bash
# Publish files to the GitHub Release of a version: create the release with them if it does
# not exist yet, otherwise attach the ones it lacks.
#
#   scripts/publish-release.sh VERSION FILE...
#
# The Windows and macOS workflows each call this for the same commit, in whichever order
# their builds finish, so one release (tagged vVERSION) ends up carrying both platforms'
# files. Their release jobs share a concurrency group, so two calls never overlap. A re-run
# finds its files already attached and publishes nothing.
#
# Needs gh with GH_TOKEN (contents: write), and GITHUB_REPOSITORY, GITHUB_SHA and
# GITHUB_SERVER_URL as GitHub Actions sets them.
set -euo pipefail

if [ "$#" -lt 2 ]; then
  echo "usage: $0 VERSION FILE..." >&2
  exit 2
fi
version=$1
shift
tag="v$version"
repo=$GITHUB_REPOSITORY

if ! gh release view "$tag" --repo "$repo" > /dev/null 2>&1; then
  short=$(echo "$GITHUB_SHA" | cut -c1-7)
  notes="Automatic build of \`main\` at ${short} (${GITHUB_SERVER_URL}/${repo}/commit/${GITHUB_SHA}). Each platform's files are attached when its build finishes.

**Windows**

- **Installer**: \`*-setup.exe\` (per-user, no administrator prompt; optional start at sign-in).
- **Portable**: \`*-portable.zip\` (unzip and run \`cosmic-mail.exe\`).

Builds are unsigned, so Windows SmartScreen warns on first run (More info → Run anyway).

**macOS**

- **Disk image**: \`*-macos-universal.dmg\` (Apple Silicon and Intel, macOS 13 or later): open it and drag Cosmic Mail to Applications.

Builds are not notarized, so macOS blocks the first launch: open System Settings → Privacy & Security and choose Open Anyway."
  gh release create "$tag" "$@" \
    --repo "$repo" \
    --target "$GITHUB_SHA" \
    --title "Cosmic Mail ${version}" \
    --notes "$notes" \
    --latest
  echo "Created release $tag with $# file(s)."
  exit 0
fi

attached=$(gh release view "$tag" --repo "$repo" --json assets --jq '.assets[].name')
missing=()
for file in "$@"; do
  if grep -qxF -- "$(basename "$file")" <<< "$attached"; then
    echo "Release $tag already has $(basename "$file")."
  else
    missing+=("$file")
  fi
done
if [ "${#missing[@]}" -eq 0 ]; then
  echo "Release $tag already has every file (a re-run); nothing to publish."
  exit 0
fi
gh release upload "$tag" "${missing[@]}" --repo "$repo"
echo "Attached ${#missing[@]} file(s) to release $tag."
