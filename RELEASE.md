# Releasing

`Cargo.toml`'s `workspace.package.version` is the one canonical version number for
the whole project — Rust crates and the `game-koi` npm package alike.
`crates/game-koi-web/js/package.json`'s version, `Cargo.lock`, and
`package-lock.json` are all derived from it and should never be hand-edited: they are
refreshed by **building**, and by nothing else.

That is the one thing to get right about the order below. No hook refreshes them at
commit time, so the build belongs *between* the bump and the commit — bump, build,
commit all of it together. Skip it and the tagged commit carries a `Cargo.lock` still
naming the previous version, while the refresh sits in the working copy afterwards
where it is easy to never commit at all. (This is not hypothetical: the tree sat at
`0.3.0` in `Cargo.toml` and `0.2.0` in `Cargo.lock` for exactly that reason.)

## Steps

1. **Bump the version** in `Cargo.toml`:

   ```toml
   [workspace.package]
   version = "0.2.0"
   ```

2. **Refresh the derived files**, so the new version actually reaches them:

   ```
   just build-web    # core + wasm bindings + npm package: rewrites Cargo.lock and
                     # syncs js/package.json + js/package-lock.json
   ```

   `build.sh` refuses to run without the matching `wasm-bindgen` CLI installed. If you
   don't have it, the minimum is the Rust half — enough to bring `Cargo.lock` in line,
   and it needs no ALSA/libudev or wasm target:

   ```
   cargo check -p game-koi-core
   ```

   Neither upgrades anything: a resolve rewrites only what it must, so the `Cargo.lock`
   diff is the three `game-koi-*` version lines and nothing else. Upgrading dependencies
   is `cargo update`, and is not part of a release.

3. **Commit it all together** — `Cargo.toml` plus everything step 2 refreshed. Make sure
   the edits are actually saved to disk *before* you run
   `jj commit` — `jj commit -m msg` finalizes whatever's on disk right now into the
   described commit and opens a fresh empty one on top. If the edit lands after that
   command runs, it ends up in the new empty commit instead of the one you're about
   to tag, and the tag silently lies about what it points to (this has happened once
   already — see [Troubleshooting](#troubleshooting)).

   ```
   jj st               # Cargo.toml + Cargo.lock (+ js/package.json and its lock)
   jj commit -m "bump version to 0.2.0"
   jj bookmark set main -r @-
   jj git push
   ```

4. **Sanity-check what would be published** before tagging (optional, and no registry
   write — step 2 already did the build this reuses):

   ```
   just npm-dry-run    # packs the npm tarball and prints its contents
   ```

5. **Tag and push the release.** `just tag` reads the version from what's actually
   committed on `main` (not from disk), so it can't tag a version that doesn't match
   what's really there:

   ```
   just tag
   ```

6. **Watch CI.** The tag push triggers `.github/workflows/npm-publish.yml`, which
   rebuilds everything from scratch, checks the tag against the version it syncs into
   `package.json`, and publishes to npm via trusted publishing (no token involved).
   Check the Actions tab — a failed version check means the tag doesn't match what's
   committed on the tagged commit; see below.

7. **The docs site follows afterwards, not as part of this.** `docs/package.json`
   installs `game-koi` from the registry rather than from the workspace, so its range
   can only move once the publish above has actually landed. Renovate does it
   (`renovate.json`); `just sync-docs-version` is the same bump by hand, for when
   waiting is annoying. Either way it is a separate commit, and it is the one derived
   version number `build.sh` cannot touch.

## Troubleshooting

**`Cargo.lock` (or `js/package.json`) still names the previous version.** Step 2 was
skipped, or ran after the commit. Nothing about the release *breaks* — CI builds
before it reads the version, so `build.sh` stamps `package.json` correctly and the tag
check passes — but two things go quietly wrong afterwards:

- `cargo pkgid` answers from the lockfile, not the manifest, so locally it reports the
  *old* version until something builds. Anything reading the version that way (as
  `build.sh` does) is right only because a `cargo build` runs two lines above it.
- A fresh clone of the tag has a dirty tree after its first build, for a lockfile the
  tag should have shipped correct.

Fix it with the step 2 commands and commit the result. A follow-up commit is fine —
nothing that was published changes, so there is no need to re-tag or re-release.

**CI says "tag vX.Y.Z does not match package.json version ...".** The tag points at
a commit whose `Cargo.toml` doesn't actually have the version the tag claims — almost
always because the version-bump commit ended up empty (see step 2 above). Since the
publish never happened (CI fails before `npm publish` runs), the safest fix is to
leave the bad tag where it is and release under the *next* version instead — don't
try to reuse it:

```
# edit Cargo.toml to the next version, e.g. 0.2.1
jj commit -m "bump version to 0.2.1"
jj bookmark set main -r @-
jj git push
just tag
```

Moving an already-pushed tag to point somewhere else is possible
(`jj tag set vX.Y.Z -r main --allow-move` + `git push origin vX.Y.Z --force`, since
`jj git push` refuses to push tags at all) but rewrites a public ref, so only reach
for it if you specifically need that exact version number to exist and are sure
nothing has already fetched the old tag.

**Manual npm publish, without CI.** For the very first publish, before a trusted
publisher can be configured on npmjs.com, or any other one-off:

```
just npm-publish
```

Needs `npm login` with publish rights to `game-koi` first.
