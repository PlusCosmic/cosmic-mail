#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat <<'EOF'
Promote the current Cosmic Mail source tree to the local daily installation.

Usage:
  scripts/promote-local.sh [--allow-dirty]
  scripts/promote-local.sh --list
  scripts/promote-local.sh --activate <release-id>

The daily installation lives under $XDG_DATA_HOME/cosmic-mail-daily (normally
~/.local/share/cosmic-mail-daily). Nothing updates it until this command is run
again. --allow-dirty is required when promoting uncommitted source.
EOF
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
install_root="$data_home/cosmic-mail-daily"
releases_dir="$install_root/releases"
applications_dir="$data_home/applications"
desktop_file="$applications_dir/dev.pluscosmic.mail.desktop"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
user_units_dir="$config_home/systemd/user"
service_name="cosmic-mail.service"
service_file="$user_units_dir/$service_name"

allow_dirty=false
action=promote
release_to_activate=

while (($#)); do
  case "$1" in
    --allow-dirty)
      allow_dirty=true
      ;;
    --list)
      action=list
      ;;
    --activate)
      action=activate
      shift
      if (($# == 0)); then
        echo "--activate requires a release ID" >&2
        exit 2
      fi
      release_to_activate="$1"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

list_releases() {
  if [[ ! -d "$releases_dir" ]]; then
    echo "No local daily releases are installed."
    return
  fi

  local current=
  if [[ -L "$install_root/current" ]]; then
    current="$(basename "$(readlink -f "$install_root/current")")"
  fi

  local found=false
  while IFS= read -r release; do
    found=true
    if [[ "$release" == "$current" ]]; then
      printf '* %s (active)\n' "$release"
    else
      printf '  %s\n' "$release"
    fi
  done < <(find "$releases_dir" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort -V)

  if [[ "$found" == false ]]; then
    echo "No local daily releases are installed."
  fi
}

activate_release() {
  local release_id="$1"
  local release_dir="$releases_dir/$release_id"
  if [[ ! -x "$release_dir/cosmic-mail" ]]; then
    echo "Release does not exist or is incomplete: $release_id" >&2
    exit 1
  fi

  ln -sfn "$release_dir" "$install_root/current.next"
  mv -Tf "$install_root/current.next" "$install_root/current"
  echo "Cosmic Mail daily is now on $release_id"
}

restart_background_service() {
  if command -v systemctl >/dev/null 2>&1 && [[ -f "$service_file" ]]; then
    systemctl --user daemon-reload
    systemctl --user restart "$service_name"
  fi
}

if [[ "$action" == list ]]; then
  list_releases
  exit 0
fi

if [[ "$action" == activate ]]; then
  activate_release "$release_to_activate"
  restart_background_service
  exit 0
fi

cd "$repo_root"

if [[ -n "$(git status --porcelain --untracked-files=normal)" && "$allow_dirty" == false ]]; then
  cat >&2 <<'EOF'
The working tree has uncommitted changes. Commit the release candidate first,
or pass --allow-dirty to make an explicitly non-reproducible local promotion.
EOF
  exit 1
fi

version="$(node -e "const fs=require('fs'); process.stdout.write(JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8')).version)")"
revision="$(git rev-parse --short=10 HEAD 2>/dev/null || printf 'no-git')"
release_id="v${version}-${revision}"
if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
  release_id+="-dirty-$(date -u +%Y%m%dT%H%M%SZ)"
fi

npm run check
npm run build
(
  cd src-tauri
  cargo check
  cargo clippy -- -D warnings
  cargo test --lib
  cargo fmt --check
)

# --no-bundle produces a standalone application binary with the frontend embedded,
# without downloading AppImage tooling or installing system packages.
npm run tauri -- build --no-bundle

release_dir="$releases_dir/$release_id"
if [[ -e "$release_dir" ]]; then
  echo "Release already exists: $release_id" >&2
  exit 1
fi

install -Dm755 src-tauri/target/release/cosmic-mail "$release_dir/cosmic-mail"
install -Dm644 src-tauri/icons/128x128.png "$release_dir/cosmic-mail.png"
release_pending=true

mkdir -p "$applications_dir"
desktop_tmp="$(mktemp --suffix=.desktop "$applications_dir/.dev.pluscosmic.mail.XXXXXX")"
mkdir -p "$user_units_dir"
service_tmp="$(mktemp "$user_units_dir/.cosmic-mail.service.XXXXXX")"
cleanup_pending_install() {
  rm -f "$desktop_tmp"
  rm -f "$service_tmp"
  if [[ "$release_pending" == true ]]; then
    rm -rf "$release_dir"
  fi
}
trap cleanup_pending_install EXIT
printf '%s\n' \
  '[Desktop Entry]' \
  'Version=1.0' \
  'Type=Application' \
  'Name=Cosmic Mail' \
  'Comment=Native mail client for Omarchy' \
  "Exec=uwsm-app -- \"$install_root/current/cosmic-mail\"" \
  "Icon=$install_root/current/cosmic-mail.png" \
  'Terminal=false' \
  'Categories=Network;Email;' \
  'StartupNotify=true' \
  'StartupWMClass=cosmic-mail' > "$desktop_tmp"
chmod 0644 "$desktop_tmp"

printf '%s\n' \
  '[Unit]' \
  'Description=Cosmic Mail background sync and notifications' \
  'After=graphical-session.target' \
  'PartOf=graphical-session.target' \
  '' \
  '[Service]' \
  'Type=simple' \
  "ExecStart=\"$install_root/current/cosmic-mail\" --background" \
  'Restart=always' \
  'RestartSec=30' \
  '' \
  '[Install]' \
  'WantedBy=graphical-session.target' > "$service_tmp"
chmod 0644 "$service_tmp"

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$desktop_tmp"
fi

mv -f "$desktop_tmp" "$desktop_file"
mv -f "$service_tmp" "$service_file"
activate_release "$release_id"
release_pending=false
systemctl --user daemon-reload
systemctl --user enable "$service_name"
systemctl --user restart "$service_name"
trap - EXIT

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$applications_dir"
fi
if command -v omarchy >/dev/null 2>&1; then
  omarchy restart walker
fi

echo
echo "Installed launcher: $desktop_file"
echo "Installed service:  $service_file"
echo "Installed release:  $release_dir"
echo "Background sync is active; launch 'Cosmic Mail' from Walker to show or focus it."
echo "This install changes only on promotion."
