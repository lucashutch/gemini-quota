#!/bin/sh
# Install the latest LimitWatch Linux x86_64 release.
set -eu

repository="lucashutch/limitwatch"
install_dir="${LIMITWATCH_INSTALL_DIR:-$HOME/.local/bin}"
release_url="https://github.com/$repository/releases/latest/download"

if [ "$(uname -s)" != "Linux" ]; then
    echo "LimitWatch release binaries are currently available for Linux only." >&2
    exit 1
fi

case "$(uname -m)" in
    x86_64 | amd64) ;;
    *)
        echo "LimitWatch release binaries are currently available for Linux x86_64 only." >&2
        exit 1
        ;;
esac

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT HUP INT TERM
binary="$tmpdir/limitwatch"
checksum="$tmpdir/limitwatch.sha256"

echo "Downloading the latest LimitWatch release..."
curl --fail --location --silent --show-error \
    "$release_url/limitwatch-linux-x86_64" -o "$binary"
curl --fail --location --silent --show-error \
    "$release_url/limitwatch-linux-x86_64.sha256" -o "$checksum"

expected=$(awk '{print $1}' "$checksum")
actual=$(sha256sum "$binary" | awk '{print $1}')
if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
    echo "Downloaded binary checksum did not match the release checksum." >&2
    exit 1
fi

mkdir -p "$install_dir"
install -m 755 "$binary" "$install_dir/limitwatch"

echo "Installed LimitWatch to $install_dir/limitwatch"
case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "Add $install_dir to your PATH to run limitwatch." >&2 ;;
esac
