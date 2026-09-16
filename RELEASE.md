# Releasing

`Cargo.toml`'s `workspace.package.version` is the one canonical version number for
the whole project — Rust crates and the `game-koi` npm package alike.
`crates/game-koi-web/js/package.json`'s version, `Cargo.lock`, and
`package-lock.json` are all derived from it and should never be hand-edited; they get
refreshed as a side effect of building.

## Steps

1. **Bump the version** in `Cargo.toml`:

   ```toml
   [workspace.package]
   version = "0.2.0"
   ```

2. **Commit it**, making sure the edit is actually saved to disk *before* you run
   `jj commit` — `jj commit -m msg` finalizes whatever's on disk right now into the
   described commit and opens a fresh empty one on top. If the edit lands after that
   command runs, it ends up in the new empty commit instead of the one you're about
   to tag, and the tag silently lies about what it points to (this has happened once
   already — see [Troubleshooting](#troubleshooting)).

   ```
   jj commit -m "bump version to 0.2.0"
   jj bookmark set main -r @-
   jj git push
   ```

3. **Sanity-check the build locally** before tagging (optional but cheap):

   ```
   just build-web      # builds core + wasm bindings + npm package, syncs package.json
   just npm-dry-run     # packs the npm tarball and prints its contents — no registry write
   ```

4. **Tag and push the release.** `just tag` reads the version from what's actually
   committed on `main` (not from disk), so it can't tag a version that doesn't match
   what's really there:

   ```
   just tag
   ```

5. **Watch CI.** The tag push triggers `.github/workflows/npm-publish.yml`, which
   rebuilds everything from scratch, checks the tag against the version it syncs into
   `package.json`, and publishes to npm via trusted publishing (no token involved).
   Check the Actions tab — a failed version check means the tag doesn't match what's
   committed on the tagged commit; see below.

## Troubleshooting

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
