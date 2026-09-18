#!/bin/sh

set -eu

workspace_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$workspace_root"

skip_build=false
if [ "${1:-}" = "--skip-build" ]; then
	skip_build=true
	shift
fi
if [ "$#" -ne 0 ]; then
	echo "Usage: scripts/package-macos-release.sh [--skip-build]" >&2
	exit 1
fi

target="aarch64-apple-darwin"
if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
	echo "The macOS release must be built natively on Apple Silicon." >&2
	exit 1
fi
version=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "notagent") | .version')
if [ -z "$version" ] || [ "$version" = "null" ]; then
	echo "Could not determine the notagent version." >&2
	exit 1
fi

dist_dir="target/dist/v$version"
archive_name="notagent-v$version-$target.tar.gz"
archive_path="$dist_dir/$archive_name"
binary_path="${NOTAGENT_RELEASE_BINARY:-target/release/notagent}"
staging_dir=$(mktemp -d "${TMPDIR:-/tmp}/notagent-release.XXXXXX")
trap 'rm -rf "$staging_dir"' EXIT

if [ "$skip_build" = false ]; then
	cargo build --release -p notagent --bin notagent
fi
if ! file "$binary_path" | rg -q 'Mach-O 64-bit executable arm64'; then
	echo "Release binary is not an arm64 Mach-O executable." >&2
	exit 1
fi
# Packaging must not turn a local, unsigned build into a public release.
codesign --verify --strict --check-notarization -R=notarized "$binary_path"

mkdir -p "$dist_dir" target/release-site/api
cp "$binary_path" "$staging_dir/notagent"
chmod 755 "$staging_dir/notagent"
touch -t 198001010000 "$staging_dir/notagent"
COPYFILE_DISABLE=1 tar --format=ustar -cf "$staging_dir/notagent.tar" -C "$staging_dir" notagent
gzip -n -c "$staging_dir/notagent.tar" > "$archive_path"
sha256=$(shasum -a 256 "$archive_path" | awk '{print $1}')

printf '%s  %s\n' "$sha256" "$archive_name" > "$archive_path.sha256"
printf '{"version":"%s"}\n' "$version" > target/release-site/api/latest-version.json
if [ ! -f target/release-site/api/models/manifest.json ]; then
	echo "Static model catalog is missing; run the model catalog importer first." >&2
	exit 1
fi
api_archive_name="notagent-api-v$version.tar.gz"
api_archive_path="$dist_dir/$api_archive_name"
cp -R target/release-site/api "$staging_dir/api"
find "$staging_dir/api" -exec touch -t 198001010000 {} +
COPYFILE_DISABLE=1 tar --format=ustar -cf "$staging_dir/api.tar" -C "$staging_dir" api
gzip -n -c "$staging_dir/api.tar" > "$api_archive_path"
api_sha256=$(shasum -a 256 "$api_archive_path" | awk '{print $1}')
printf '%s  %s\n' "$api_sha256" "$api_archive_name" > "$api_archive_path.sha256"

cat > "$dist_dir/notagent.rb" <<EOF
cask "notagent" do
  version "$version"
  sha256 "$sha256"

  url "https://github.com/notagentdev/notagent/releases/download/v#{version}/notagent-v#{version}-aarch64-apple-darwin.tar.gz"
  name "notagent"
  desc "Terminal coding agent with multi-provider model support"
  homepage "https://notagent.dev/"

  livecheck do
    url "https://github.com/notagentdev/notagent"
    strategy :github_latest
  end

  depends_on arch: :arm64

  binary "notagent"
end
EOF

echo "Archive: $archive_path"
echo "SHA-256: $sha256"
echo "Cask: $dist_dir/notagent.rb"
echo "Static API: $api_archive_path"
echo "Static API SHA-256: $api_sha256"
