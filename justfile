# Run `just` with no arguments to list these.
default:
    @just --list

# --- desktop ---------------------------------------------------------------

# Play a ROM in a window (release build — debug is far too slow).
run rom:
    cargo run --release -- {{ rom }}

# Release build of the desktop binary.
build:
    cargo build --release

# Unit tests. Pass a substring to filter, e.g. `just test cpu_instrs`.
test filter="":
    cargo test {{ filter }}

# `just test`, but with println!/tracing output visible.
test-verbose filter="":
    cargo test {{ filter }} -- --nocapture

# All unit tests plus the Blargg/Mooneye ROM suites (release, or they crawl).
test-roms filter="":
    cargo test --release {{ filter }}

lint:
    cargo clippy --all-targets

fmt:
    cargo fmt

# --- browser / npm package ---------------------------------------------------

# The core must keep building for wasm32 unchanged; check that after touching it.
check-wasm:
    cargo build -p game-koi-core --target wasm32-unknown-unknown

# Build the core, the wasm-bindgen glue, and the game-koi npm package.
build-web:
    ./crates/game-koi-web/build.sh

# Serve the browser demo at http://localhost:8080/web/.
serve-web: build-web
    python3 -m http.server -d crates/game-koi-web 8080

# Build, pack, and print what `npm publish` would do — no registry write.
[working-directory: 'crates/game-koi-web/js']
npm-dry-run: build-web
    npm publish --access public --dry-run

# CI does releases via trusted publishing on a tag push (see
# .github/workflows/npm-publish.yml); this is for a manual one-off or the very first
# publish, before a trusted publisher can be configured on npmjs.com.
#
# Publish game-koi to npm for real, from this machine (needs `npm login`).
[working-directory: 'crates/game-koi-web/js']
npm-publish: build-web
    npm publish --access public

# Tag Cargo.toml's current version as a release and push it (bump the version first).
tag:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(cargo pkgid -p game-koi-web | sed 's/.*#//')
    jj tag set "v$version" -r main
    git push origin "v$version"
