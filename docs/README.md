# game-koi docs

The documentation site for [`game-koi`](https://www.npmjs.com/package/game-koi), built
with [Astro](https://astro.build) + [Starlight](https://starlight.astro.build) and hosted
on readthedocs.com.

Astro was chosen over Sphinx/MkDocs because the example pages embed real, running
JavaScript: the ROM player is the published `game-koi` package hydrated as a Svelte
island, and the Three.js page is a plain client-side script. Astro's islands let both
live on the same static site without either becoming a single-page app.

readthedocs.com builds this through `build.commands` in the repo's `.readthedocs.yaml`
rather than a Sphinx/MkDocs builder — it runs `npm ci && npm run build` here and serves
`dist/`.

## Commands

From the repo root, `just build-docs` and `just serve-docs`. Or directly:

| Command           | Does                                              |
| :---------------- | :------------------------------------------------ |
| `npm run dev`     | dev server at `localhost:4321`                     |
| `npm run build`   | production build into `dist/`                      |
| `npm run preview` | serve `dist/` — what readthedocs actually ships    |
| `npx astro check` | type-check `.astro`/`.svelte`/content collections  |

## Gotcha

`src/components/RomPlayer.svelte` imports `game-koi` **inside** its event handler, not at
module scope. Vite walks a component's static imports during `astro build`'s SSG pass even
under `client:only`, and `game-koi` touches `AudioContext`/wasm/DOM on evaluation — a
top-level import fails the build, not just the dev server.

This site depends on `game-koi` from the public npm registry, not on the crate sources in
`../crates/game-koi-web/`. Local emulator changes show up here only after a release.
