#!/usr/bin/env bash
# Build "Cosmic Mail.app" from release binaries and wrap it in a disk image. macOS only
# (lipo, sips, iconutil, codesign, hdiutil).
#
#   packaging/macos/package.sh VERSION OUT_DIR BINARY [BINARY...]
#
# One BINARY per architecture; two are merged into a universal binary. Writes
# OUT_DIR/cosmic-mail-VERSION-macos-{universal,arm64,x86_64}.dmg and prints its path.
#
# Run by .github/workflows/macos.yml; by hand, for a build of this Mac's architecture:
#   cargo build --release
#   packaging/macos/package.sh "$(scripts/version.sh)" target/macos target/release/cosmic-mail
set -euo pipefail

if [ "$#" -lt 3 ]; then
  echo "usage: $0 VERSION OUT_DIR BINARY [BINARY...]" >&2
  exit 2
fi
version=$1
out_dir=$2
shift 2

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The disk image's contents: the app, and a link to drag it onto.
app="$work/image/Cosmic Mail.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
ln -s /Applications "$work/image/Applications"

exe="$app/Contents/MacOS/cosmic-mail"
if [ "$#" -eq 1 ]; then
  cp "$1" "$exe"
else
  lipo -create -output "$exe" "$@"
fi
chmod 755 "$exe"

case "$(lipo -archs "$exe")" in
  *x86_64*arm64* | *arm64*x86_64*) arch=universal ;;
  *arm64*) arch=arm64 ;;
  *x86_64*) arch=x86_64 ;;
  *)
    echo "unexpected architectures in $exe: $(lipo -archs "$exe")" >&2
    exit 1
    ;;
esac

sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" > /dev/null

# AppIcon.icns from the full-size icon, at every size the Finder and Dock ask for.
iconset="$work/AppIcon.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$root/icon.png" --out "$iconset/icon_${size}x${size}.png" > /dev/null
  sips -z "$((size * 2))" "$((size * 2))" "$root/icon.png" \
    --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil --convert icns --output "$app/Contents/Resources/AppIcon.icns" "$iconset"

# An ad-hoc signature over the whole bundle. Apple Silicon runs nothing unsigned, and
# without it a downloaded copy is reported as damaged rather than merely unidentified.
codesign --force --sign - "$app"
codesign --verify --strict "$app"

mkdir -p "$out_dir"
dmg="$out_dir/cosmic-mail-$version-macos-$arch.dmg"
# hdiutil now and then fails with "Resource busy" on CI runners; a retry gets through.
for attempt in 1 2 3; do
  if hdiutil create -volname "Cosmic Mail" -srcfolder "$work/image" \
    -fs HFS+ -format UDZO -ov "$dmg" > /dev/null; then
    break
  fi
  if [ "$attempt" -eq 3 ]; then
    echo "hdiutil could not create $dmg" >&2
    exit 1
  fi
  sleep 5
done
echo "$dmg"
