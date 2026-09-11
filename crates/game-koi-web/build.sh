#!/usr/bin/env bash
# Builds the browser frontend into web/pkg/.
#
# wasm-bindgen's CLI and the wasm-bindgen crate must be the SAME version or the
# generated glue will not match the module's ABI, so the version is pinned here and in
# Cargo.toml together. The error when they disagree does not say so clearly.
set -euo pipefail

VERSION=0.2.128
cd "$(dirname "$0")/../.."

if ! command -v wasm-bindgen >/dev/null; then
  echo "wasm-bindgen not found. Install the matching version with:" >&2
  echo "  cargo install wasm-bindgen-cli --version $VERSION" >&2
  exit 1
fi

have=$(wasm-bindgen --version | awk '{print $2}')
if [ "$have" != "$VERSION" ]; then
  echo "wasm-bindgen CLI is $have but the crate is pinned to $VERSION." >&2
  echo "  cargo install wasm-bindgen-cli --version $VERSION --force" >&2
  exit 1
fi

cargo build -p game-koi-web --release --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript \
  --out-dir crates/game-koi-web/web/pkg \
  target/wasm32-unknown-unknown/release/game_koi_web.wasm

echo "built. serve it with:"
echo "  python3 -m http.server -d crates/game-koi-web/web 8080"
