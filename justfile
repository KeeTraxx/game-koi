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

# The npm package's own tests (node's test runner, against the compiled dist/).
[working-directory: 'crates/game-koi-web/js']
test-web:
    npm test

# Serve the browser demo at http://localhost:8080/.
#
# The document root is the crate itself: index.html sits there next to js/, so the demo
# is at `/` and the built package it imports is reachable at /js/dist/. Rooting the
# server any deeper breaks that import — a static server will not serve a path above its
# own root, and the failure looks like a blank page with one console line, not an error.
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

# --- docs --------------------------------------------------------------------

# Build the Astro/Starlight docs site (docs/dist/).
[working-directory: 'docs']
build-docs:
    npm install --no-audit --no-fund
    npm run build

# Build the docs and preview the built output at http://localhost:4321/.
#
# Runs `astro preview` against docs/dist rather than `astro dev`, so this is what a
# readthedocs.com build actually ships — catches issues (like a static top-level
# `import` of `game-koi` sneaking into an island) that only show up post-build, not in
# the dev server.
[working-directory: 'docs']
serve-docs: build-docs
    npm run preview

# Tag main's committed version and push it (bump the version and commit it first).
tag:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(jj file show -r main Cargo.toml | grep -m1 '^version = ' | cut -d'"' -f2)
    jj tag set "v$version" -r main
    git push origin "v$version"
