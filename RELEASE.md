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

2. **Run `just release`.** That is everything mechanical — refresh, verify, commit,
   push, tag — in the order that keeps the derived files inside the tagged commit:

   ```
   just release        # `just release yes` skips the confirmation prompt
   ```

   It **refuses before touching anything** if:

   - the version is already on npm. A published version number is gone forever — npm
     never takes one twice, not even after an unpublish — so this is the one mistake
     worth catching early, and it is the mistake 0.4.0 ran into.
   - the tag `vX.Y.Z` already exists on `origin`.
   - `main` isn't an ancestor of the working copy (releasing sideways off an unmerged
     line of work).
   - `Cargo.toml` is unchanged (you skipped step 1), or the working copy contains
     *anything* beyond the bump and its derived files — otherwise an unrelated change
     gets swept into "chore: bump to X" where nobody will look for it.

   Then, in order: `just build-web` (which is what refreshes `Cargo.lock`,
   `js/package.json` and its lockfile), `cargo test --workspace` and `just test-web`
   (what the publish workflow runs — better to fail here than after the tag has spent
   the version number), a diff for you to confirm, and finally the commit, `jj git
   push`, and `just tag`.

3. **Watch CI.** The tag push triggers `.github/workflows/npm-publish.yml`, which
   rebuilds everything from scratch, checks the tag against the version it syncs into
   `package.json`, and publishes to npm via trusted publishing (no token involved).
   Check the Actions tab — a failed version check means the tag doesn't match what's
   committed on the tagged commit; see below.

4. **The docs site follows afterwards, not as part of this.** `docs/package.json`
   installs `game-koi` from the registry rather than from the workspace, so its range
   can only move once the publish above has actually landed. Renovate does it
   (`renovate.json`); `just sync-docs-version` is the same bump by hand, for when
   waiting is annoying. Either way it is a separate commit, and it is the one derived
   version number `build.sh` cannot touch.

## Doing it by hand

What `just release` runs, for when it fails halfway or you want to do a step yourself.

**Refresh the derived files.** `just build-web` is the whole of it. `build.sh` refuses to
run without the matching `wasm-bindgen` CLI installed; without it, the minimum is the
Rust half — enough to bring `Cargo.lock` in line, and it needs no ALSA/libudev or wasm
target:

```
cargo check -p game-koi-core
```

Neither upgrades anything: a resolve rewrites only what it must, so the `Cargo.lock` diff
is the three `game-koi-*` version lines and nothing else. Upgrading dependencies is
`cargo update`, and is not part of a release.

**Commit it all together** — `Cargo.toml` plus everything the build refreshed. Make sure
the edits are actually saved to disk *before* you run `jj commit`: it finalizes whatever's
on disk right now into the described commit and opens a fresh empty one on top. If an edit
lands after that command runs, it ends up in the new empty commit instead of the one
you're about to tag, and the tag silently lies about what it points to (this has happened
once already — see [Troubleshooting](#troubleshooting)).

```
jj st               # Cargo.toml + Cargo.lock (+ js/package.json and its lock)
jj commit -m "chore: bump to 0.2.0"
jj bookmark set main -r @-
jj git push
```

**Check what would be published**, optionally — no registry write, and it reuses the
build above:

```
just npm-dry-run    # packs the npm tarball and prints its contents
```

**Tag and push.** `just tag` reads the version from what's actually committed on `main`
(not from disk), so it can't tag a version that doesn't match what's really there:

```
just tag
```

## Troubleshooting

**`Cargo.lock` (or `js/package.json`) still names the previous version.** The build was
skipped, or ran after the commit — which `just release` cannot do, so this means a
hand-rolled release. Nothing about it *breaks* — CI builds
before it reads the version, so `build.sh` stamps `package.json` correctly and the tag
check passes — but two things go quietly wrong afterwards:

- `cargo pkgid` answers from the lockfile, not the manifest, so locally it reports the
  *old* version until something builds. Anything reading the version that way (as
  `build.sh` does) is right only because a `cargo build` runs two lines above it.
- A fresh clone of the tag has a dirty tree after its first build, for a lockfile the
  tag should have shipped correct.

Fix it with the commands under [Doing it by hand](#doing-it-by-hand) and commit the
result. A follow-up commit is fine —
nothing that was published changes, so there is no need to re-tag or re-release.

**CI says "tag vX.Y.Z does not match package.json version ...".** The tag points at
a commit whose `Cargo.toml` doesn't actually have the version the tag claims — almost
always because the version-bump commit ended up empty (see the `jj commit` note under
[Doing it by hand](#doing-it-by-hand)). Since the
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
