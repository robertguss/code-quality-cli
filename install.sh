#!/bin/sh
# Install a prebuilt qgate binary from GitHub Releases.
#
#   curl -fsSL https://github.com/robertguss/qgate/releases/latest/download/install.sh | sh
#
# Environment:
#   QGATE_VERSION      tag to install, e.g. v0.2.0 (default: latest release)
#   QGATE_INSTALL_DIR  where to put the binary (default: $HOME/.local/bin)
set -eu

repo=robertguss/qgate
version=${QGATE_VERSION:-latest}
dir=${QGATE_INSTALL_DIR:-$HOME/.local/bin}

case "$(uname -s)" in
  Linux) os=unknown-linux-musl ;;
  Darwin) os=apple-darwin ;;
  *) echo "qgate: unsupported OS $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) echo "qgate: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

asset=qgate-$arch-$os.tar.gz
if [ "$version" = latest ]; then
  base=https://github.com/$repo/releases/latest/download
else
  base=https://github.com/$repo/releases/download/$version
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$base/$asset" -o "$tmp/$asset"
curl -fsSL "$base/$asset.sha256" -o "$tmp/$asset.sha256"

expected=$(cut -d' ' -f1 "$tmp/$asset.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
fi
if [ "$expected" != "$actual" ]; then
  echo "qgate: checksum mismatch for $asset" >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/qgate" "$dir/qgate"
echo "qgate installed to $dir/qgate"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "note: $dir is not on PATH" >&2 ;;
esac
