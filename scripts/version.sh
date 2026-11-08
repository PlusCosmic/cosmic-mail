#!/usr/bin/env bash
# Print the version of the checked-out commit: MAJOR.MINOR from Cargo.toml,
# PATCH = the number of commits reachable from HEAD.
#
# Every merge to main adds commits, so the patch only ever grows, and every
# workflow (Windows release, Arch package) computes the same number for the
# same commit with no stored counter. Bump MAJOR.MINOR in Cargo.toml by hand
# for a release people should notice; the patch carries on counting.
#
# Needs full history: in GitHub Actions, check out with fetch-depth: 0.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

base=$(sed -n 's/^version = "\([0-9][0-9]*\.[0-9][0-9]*\)\..*"$/\1/p' Cargo.toml | head -1)
[ -n "$base" ] || { echo "no MAJOR.MINOR version in Cargo.toml" >&2; exit 1; }

if [ "$(git rev-parse --is-shallow-repository)" = "true" ]; then
  echo "shallow clone: the commit count would be wrong (fetch full history)" >&2
  exit 1
fi
echo "$base.$(git rev-list --count HEAD)"
