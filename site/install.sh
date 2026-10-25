#!/bin/sh
# Installs Briardr from its latest GitHub release (https://briardr.dev/install.sh):
#
#   curl -fsSL https://briardr.dev/install.sh | sh
#
# It downloads the archive for this computer (macOS or Linux, on Apple silicon/aarch64 or x86_64), checks
# it against the release's SHA256SUMS, puts Briardr in ~/.local/lib/briardr and links `briardr` into
# ~/.local/bin. Then `briardr setup` checks for Herdr, offers to install the version Briardr is tested
# with, and says what's next. Run it again to upgrade. Arguments go to `briardr setup`
# (`sh -s -- --yes` installs Herdr without asking).
#   BRIARDR_VERSION=0.2.0       that version instead of the latest
#   BRIARDR_INSTALL_DIR=<dir>   somewhere else than ~/.local/lib/briardr
set -eu

repo=https://github.com/ChrisSku/briardr
dir=${BRIARDR_INSTALL_DIR:-$HOME/.local/lib/briardr}
version=${BRIARDR_VERSION:-}
version=${version#v}
# For testing: where the archives and SHA256SUMS are, instead of the GitHub release.
base=${BRIARDR_DOWNLOAD_URL:-}

say() { printf '%s\n' "$*"; }
die() {
  printf 'Briardr install: %s\n' "$*" >&2
  exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

case $(uname -s) in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) die "Briardr runs on macOS and Linux, not $(uname -s)." ;;
esac
case $(uname -m) in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) die "Briardr is built for aarch64 and x86_64, not $(uname -m)." ;;
esac
# A shell running under Rosetta reports x86_64 on Apple silicon.
if [ "$os" = macos ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
  arch=aarch64
fi
if [ "$os" = linux ]; then
  glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
  [ -n "$glibc" ] || die "Briardr's Linux build needs glibc 2.39 or newer (Ubuntu 24.04 or later); this system has no glibc."
  awk -v v="$glibc" 'BEGIN { split(v, n, "."); exit !(n[1] > 2 || (n[1] == 2 && n[2] >= 39)) }' ||
    die "Briardr's Linux build needs glibc 2.39 or newer (Ubuntu 24.04 or later); this system has $glibc."
fi

if have curl; then
  # Only HTTPS, also after GitHub's redirect to its storage (anything goes for a test's BRIARDR_DOWNLOAD_URL).
  if [ -n "$base" ]; then proto='=https,http,file'; else proto='=https'; fi
  fetch() { curl -fsSL --proto "$proto" --proto-redir "$proto" "$1" -o "$2"; }
elif have wget; then
  fetch() { wget -q "$1" -O "$2"; }
else
  die "it needs curl or wget to download Briardr."
fi
if have sha256sum; then
  sha256() { sha256sum "$1" | awk '{print $1}'; }
elif have shasum; then
  sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
else
  die "it needs sha256sum or shasum to check the download."
fi

# An upgrade replaces the folder, so it must be a Briardr installation.
if [ -e "$dir" ] && [ ! -x "$dir/bin/briardr" ]; then
  die "$dir exists and isn't a Briardr installation. Set BRIARDR_INSTALL_DIR to another folder."
fi
if [ -z "$base" ]; then
  if [ -n "$version" ]; then base=$repo/releases/download/v$version; else base=$repo/releases/latest/download; fi
fi
asset=briardr-$os-$arch.tar.gz
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "Downloading Briardr ${version:-(the latest release)} for ${os} on ${arch}…"
fetch "$base/$asset" "$tmp/$asset" || die "couldn't download $base/$asset."
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || die "couldn't download $base/SHA256SUMS."
want=$(awk -v f="$asset" '$2 == f {print $1}' "$tmp/SHA256SUMS")
[ -n "$want" ] || die "the release's SHA256SUMS doesn't list $asset."
[ "$(sha256 "$tmp/$asset")" = "$want" ] || die "$asset doesn't match the release's checksum. Nothing was installed."
mkdir "$tmp/unpacked"
tar -xzf "$tmp/$asset" -C "$tmp/unpacked"
new=$(find "$tmp/unpacked" -mindepth 1 -maxdepth 1 -type d -name 'briardr-*' | head -n 1)
[ -n "$new" ] && [ -x "$new/bin/briardr" ] || die "the archive has no bin/briardr."

# The new version goes next to the installed one, then takes its place. A running Briardr keeps running
# the old one until it restarts.
mkdir -p "$(dirname "$dir")"
rm -rf "$dir.new" "$dir.old"
mv "$new" "$dir.new"
if [ -e "$dir" ]; then mv "$dir" "$dir.old"; fi
mv "$dir.new" "$dir"
rm -rf "$dir.old"
say "Installed $("$dir/bin/briardr" --version) in $dir."
"$dir/bin/briardr" install-cli

plist=$HOME/Library/LaunchAgents/app.briardr.server.plist
if [ -f "$plist" ] && grep -q -e "$dir/bin/briardr" -e "$HOME/.local/bin/briardr" "$plist"; then
  say "Restart Briardr to run this version: launchctl kickstart -k gui/$(id -u)/app.briardr.server"
fi

echo
# Setup asks before installing Herdr, so it gets the terminal even when this script came through a pipe.
if [ -t 1 ] && { : </dev/tty; } 2>/dev/null; then
  "$dir/bin/briardr" setup "$@" </dev/tty || true
else
  "$dir/bin/briardr" setup "$@" || true
fi
