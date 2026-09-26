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

# Re-export the Game Boy model the Three.js example loads, after editing the .blend.
#
# The .glb is committed, not built: readthedocs and CI have no Blender, so this is the
# one docs asset that only regenerates on a machine that does. Run it and commit the
# result alongside the .blend change.
export-model:
    blender -b gb-model/gb_model.blend --python gb-model/export.py -- docs/src/assets/gb_model.glb

# Point the docs site at the current release of the game-koi npm package.
#
# Renovate normally does this on its own (see renovate.json — it is the dependency that
# config exists for); this is the impatient path, for when a release just went out and
# the docs should reflect it now rather than at Renovate's next run.
#
# docs/ consumes `game-koi` from the registry rather than from crates/game-koi-web/js —
# that is what makes its ROM-player page a test of the *published* package — so this is
# the one version number in the repo that build.sh cannot sync, and the only one that
# has to wait: it can only be moved once that version is actually on npm, i.e. after
# the publish workflow has finished for the tag. The version still comes from
# Cargo.toml, so the range is never hand-typed.
#
# Read out of the manifest with grep rather than `cargo pkgid` (which build.sh uses):
# pkgid answers from Cargo.lock, so it reports the *previous* version until something
# has built since the bump. build.sh gets away with it because a cargo build runs two
# lines above; here there is nothing to refresh the lock, and the failure would be a
# silently stale pin rather than an error.
#
# Why this exists at all: a caret range on a 0.x version admits patch bumps only
# (^0.2.0 means >=0.2.0 <0.3.0), so every *minor* release silently leaves the docs on
# the old line. That is how docs/package.json came to sit on 0.2.0 at 0.3.0.
[doc("Point the docs site at the current published game-koi release")]
sync-docs-version:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)
    # --prefer-online because npm's metadata cache will happily report a release that
    # landed minutes ago as nonexistent, and the check would then be backwards.
    if ! npm view --prefer-online "game-koi@$version" version >/dev/null 2>&1; then
      echo "game-koi@$version is not on the registry yet." >&2
      echo "Publish it first (push the v$version tag), then run this again." >&2
      exit 1
    fi
    cd docs
    npm pkg set dependencies.game-koi="^$version"
    # The lockfile has to move with it: readthedocs builds with `npm ci`, which refuses
    # outright when the lock and package.json disagree rather than resolving anew.
    npm install --package-lock-only --no-audit --no-fund
    echo
    echo "docs now ask for game-koi ^$version."
    echo "Commit docs/package.json and docs/package-lock.json."

# --- releasing ----------------------------------------------------------------

# Cut a release: everything after the version bump (RELEASE.md steps 2-5).
#
# Step 1 stays yours — edit `workspace.package.version` in Cargo.toml, then run this.
# It refreshes the derived files, runs what CI would run, commits the lot as one commit,
# pushes main, and tags. That tag is what publishes to npm, so the checks up front are
# the point of the recipe as much as the automation is: a version number that reaches
# the registry can never be reused, so the cheap moment to catch a mistake is now.
#
# `just release yes` skips the confirmation prompt.
[doc("Cut a release: refresh, verify, commit, tag, push (RELEASE.md steps 2-5)")]
release confirm="":
    #!/usr/bin/env bash
    set -euo pipefail

    version=$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)
    echo "releasing $version"

    # Refuse to reuse a published version. npm reserves a version number permanently —
    # even unpublishing does not free it — so this is not recoverable after the fact,
    # only avoidable before it.
    if npm view --prefer-online "game-koi@$version" version >/dev/null 2>&1; then
      echo >&2
      echo "game-koi@$version is already on npm, and npm never takes a version twice." >&2
      echo "Bump Cargo.toml to the next version and run this again." >&2
      exit 1
    fi

    if git ls-remote --exit-code --tags origin "v$version" >/dev/null 2>&1; then
      echo "tag v$version already exists on origin — pick the next version." >&2
      exit 1
    fi

    # main has to be an ancestor of the working copy, or `jj bookmark set main -r @-`
    # below would move the bookmark sideways onto a line of work that was never merged.
    if [ -z "$(jj log --no-graph -r 'main & ::@' -T 'commit_id')" ]; then
      echo "main is not an ancestor of the working copy; rebase before releasing." >&2
      exit 1
    fi

    # Only the version bump and the files derived from it belong in this commit. Anything
    # else in the working copy would be swept into "chore: bump to X" silently, which is
    # how a release ends up carrying a change nobody reviewed.
    changed=$(jj diff --summary | awk '{print $2}')
    if ! echo "$changed" | grep -qx 'Cargo.toml'; then
      echo "Cargo.toml is unchanged — bump the version first (RELEASE.md step 1)." >&2
      exit 1
    fi
    for file in $changed; do
      case "$file" in
        Cargo.toml|Cargo.lock) ;;
        crates/game-koi-web/js/package.json|crates/game-koi-web/js/package-lock.json) ;;
        *)
          echo "unrelated change in the working copy: $file" >&2
          echo "commit or revert it before releasing." >&2
          exit 1
          ;;
      esac
    done

    # Step 2: the derived files. build.sh refuses without the matching wasm-bindgen CLI,
    # which is the right answer here — a release should ship a package that was actually
    # built, not one CI is the first to compile.
    just build-web

    # What the publish workflow runs, run before the tag rather than after it. A tag that
    # fails CI has already spent its version number.
    cargo test --workspace
    just test-web

    echo
    jj diff --stat
    echo
    if [ "{{ confirm }}" != "yes" ]; then
      read -r -p "tag and push v$version? this publishes to npm. [y/N] " reply
      case "$reply" in
        [yY]*) ;;
        *) echo "aborted — nothing committed, nothing pushed."; exit 1 ;;
      esac
    fi

    # Steps 3 and 5. `jj commit` finalizes whatever is on disk *now*, which is why the
    # build above had to finish first.
    jj commit -m "chore: bump to $version"
    jj bookmark set main -r @-
    jj git push
    just tag

    echo
    echo "tagged v$version and pushed. CI publishes it: gh run watch, or the Actions tab."
    echo 'the docs site follows once it lands — Renovate, or `just sync-docs-version`.'

# Tag main's committed version and push it (bump the version and commit it first).
#
# Usually reached through `just release` rather than run directly.
#
# The docs site is not part of this: it tracks the *published* package, so bump it with
# `just sync-docs-version` once the publish workflow has finished.
[doc("Tag main's committed version and push it — usually via `just release`")]
tag:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(jj file show -r main Cargo.toml | grep -m1 '^version = ' | cut -d'"' -f2)
    jj tag set "v$version" -r main
    git push origin "v$version"
