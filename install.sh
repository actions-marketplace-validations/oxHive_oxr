#!/bin/sh
# Installs a prebuilt oxr binary from a GitHub release.
#
#   curl -fsSL https://get.oxhive.dev/oxr | sh
#   curl -fsSL https://get.oxhive.dev/oxr | VERSION=v1.2.3 sh
#
# Environment:
#   VERSION            release tag to install (default: latest; `$1` also works)
#   INSTALL_DIR        install directory (default: ~/.local/bin)
#   OXR_SKIP_CHECKSUM  set to 1 to skip SHA-256 verification; only needed for
#                      releases before v0.2.0, which published no checksums.txt
#
# Deliberately POSIX sh (no pipefail, no bash-isms): `| sh` runs dash on
# Debian/Ubuntu.
set -eu

repo="oxhive/oxr"
version="${VERSION:-${1:-latest}}"
bin_dir="${INSTALL_DIR:-${OXR_INSTALL_DIR:-${HOME}/.local/bin}}"

die() {
  echo "error: $*" >&2
  exit 1
}

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)   target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64)  target=aarch64-unknown-linux-gnu ;;
  Linux-arm64)    target=aarch64-unknown-linux-gnu ;;
  Darwin-arm64)   target=aarch64-apple-darwin ;;
  *) die "oxr has no prebuilt binary for $(uname -s)-$(uname -m). Supported: Linux x86_64/aarch64, macOS arm64." ;;
esac

if [ "$version" = "latest" ]; then
  base="https://github.com/${repo}/releases/latest/download"
else
  base="https://github.com/${repo}/releases/download/${version}"
fi
asset="oxr-${target}.tar.gz"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

echo "Downloading ${base}/${asset}"
curl -fsSL "${base}/${asset}" -o "${tmp_dir}/${asset}" || die "download failed: ${base}/${asset}"

if [ "${OXR_SKIP_CHECKSUM:-0}" = "1" ]; then
  echo "warning: OXR_SKIP_CHECKSUM=1, not verifying ${asset}" >&2
else
  curl -fsSL "${base}/checksums.txt" -o "${tmp_dir}/checksums.txt" \
    || die "could not download ${base}/checksums.txt to verify ${asset} (releases before v0.2.0 have none; set OXR_SKIP_CHECKSUM=1 to install one anyway)"
  expected="$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1; exit }' "${tmp_dir}/checksums.txt")"
  [ -n "$expected" ] || die "checksums.txt has no entry for ${asset}"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${tmp_dir}/${asset}" | awk '{ print $1 }')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "${tmp_dir}/${asset}" | awk '{ print $1 }')"
  else
    die "need sha256sum or shasum to verify the download (or set OXR_SKIP_CHECKSUM=1)"
  fi
  [ "$expected" = "$actual" ] || die "checksum mismatch for ${asset}: expected ${expected}, got ${actual}"
fi

tar -xzf "${tmp_dir}/${asset}" -C "$tmp_dir"
chmod +x "${tmp_dir}/oxr"

if mkdir -p "$bin_dir" 2>/dev/null && [ -w "$bin_dir" ]; then
  mv "${tmp_dir}/oxr" "${bin_dir}/oxr"
else
  command -v sudo >/dev/null 2>&1 || die "${bin_dir} is not writable; set INSTALL_DIR to a writable directory"
  sudo mkdir -p "$bin_dir"
  sudo mv "${tmp_dir}/oxr" "${bin_dir}/oxr"
fi

echo "Installed $("${bin_dir}/oxr" --version) to ${bin_dir}/oxr"
case ":${PATH}:" in
  *":${bin_dir}:"*) ;;
  *) echo "note: ${bin_dir} is not on your PATH" >&2 ;;
esac
