#!/bin/bash
# Build the native binary for THIS machine and pack the launcher plus the
# matching platform package into ./dist (nothing is published).
#   npm/scripts/pack-local.sh [linux-x64|linux-arm64|darwin-arm64|darwin-x64]
set -euo pipefail
cd "$(dirname "$0")/../.."
variant=${1:-$(node -p 'process.platform + "-" + (process.arch === "x64" ? "x64" : process.arch)')}
export PATH="$HOME/.cargo/bin:$PATH"

node npm/scripts/set-version.mjs "$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
cargo build --release --locked
mkdir -p "npm/platforms/cove-cli-$variant/bin" dist
cp target/release/cove "npm/platforms/cove-cli-$variant/bin/cove"
chmod +x "npm/platforms/cove-cli-$variant/bin/cove"

npm test --prefix npm/cove-cli
npm pack "./npm/platforms/cove-cli-$variant" --pack-destination dist
npm pack ./npm/cove-cli --pack-destination dist
ls -la dist/*.tgz
