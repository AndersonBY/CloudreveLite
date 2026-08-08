#!/bin/sh
set -eu

server=''
pairing_code=''
install_dir="${HOME}/.local/bin"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --server)
      server=${2-}
      shift 2
      ;;
    --pairing-code)
      pairing_code=${2-}
      shift 2
      ;;
    --install-dir)
      install_dir=${2-}
      shift 2
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [ -z "$server" ] || [ -z "$pairing_code" ]; then
  echo 'Usage: install-cli.sh --server URL --pairing-code CODE [--install-dir DIR]' >&2
  exit 2
fi

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
  *) echo "Unsupported operating system: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64) arch=amd64 ;;
  arm64|aarch64) arch=arm64 ;;
  *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

asset="cloudreve-cli_${os}_${arch}.tar.gz"
release_base='https://github.com/AndersonBY/CloudreveLite/releases/latest/download'
temp_dir=$(mktemp -d "${TMPDIR:-/tmp}/cloudreve-cli.XXXXXX")
trap 'rm -rf "$temp_dir"' EXIT HUP INT TERM

curl -fsSL "$release_base/$asset" -o "$temp_dir/$asset"
curl -fsSL "$release_base/SHA256SUMS" -o "$temp_dir/SHA256SUMS"

expected=$(awk -v file="$asset" '$2 == file || $2 == "*" file { print $1; exit }' "$temp_dir/SHA256SUMS")
if [ -z "$expected" ]; then
  echo "No checksum was published for $asset" >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$temp_dir/$asset" | awk '{print $1}')
else
  actual=$(shasum -a 256 "$temp_dir/$asset" | awk '{print $1}')
fi
if [ "$actual" != "$expected" ]; then
  echo "Checksum verification failed for $asset" >&2
  exit 1
fi

mkdir -p "$install_dir"
tar -xzf "$temp_dir/$asset" -C "$temp_dir" cloudreve-cli
install -m 0755 "$temp_dir/cloudreve-cli" "$install_dir/cloudreve-cli"
"$install_dir/cloudreve-cli" connect --server "$server" --pairing-code "$pairing_code"

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "Add $install_dir to PATH to run cloudreve-cli directly." >&2 ;;
esac
