// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

import svelte from '@astrojs/svelte';

// readthedocs serves every version under its own path prefix — /en/latest/ for the
// default version, /en/<pr-number>/ for a pull request preview, /en/<tag>/ for a tagged
// one — so neither the origin nor the base is a constant that can be written down here.
// Getting this wrong is silent: the pages build and deploy, and every stylesheet, script
// and internal link 404s because it points at the server root rather than the prefix.
//
// READTHEDOCS_CANONICAL_URL is set during a readthedocs build and carries both halves.
// Without it — `just build-docs`, `just serve-docs`, the docs job in CI — the site is
// served from the root, which is what the local preview wants.
const canonical = new URL(
    process.env.READTHEDOCS_CANONICAL_URL ?? 'https://game-koi.readthedocs.io/',
);
// Astro takes the base without a trailing slash; undefined leaves it at the root.
const base = canonical.pathname.replace(/\/+$/, '') || undefined;

// Astro and Starlight base-correct their own output — assets, page routes, the sidebar —
// but a link written by hand is emitted exactly as typed. So **write internal links
// relative** (`../examples/rom-player/`), never root-absolute (`/examples/rom-player/`):
// the latter resolves against the server root rather than the version prefix and 404s,
// and nothing catches it, because the build passes and only a click shows the problem.
// That holds for .astro pages too. `import.meta.env.BASE_URL` is the other way, but it
// mirrors `base` and so carries **no** trailing slash: `${BASE_URL}examples/` silently
// yields `/en/latestexamples/`. Relative links avoid the whole question.
//
// Sidebar entries in this file are the exception — Starlight prefixes those itself, so
// they stay root-absolute, and making one relative would break it on every nested page.
//
// (A rehype plugin could rewrite those links centrally, but Astro 7's default Markdown
// processor does not run rehype plugins without adding @astrojs/markdown-remark back as
// a dependency, which swaps the whole pipeline to unified for the sake of six links.)

// https://astro.build/config
export default defineConfig({
    // Origin only: Astro joins this with `base` for canonical URLs and the sitemap.
    site: canonical.origin,
    base,
    integrations: [starlight({
        title: 'game-koi',
        social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/KeeTraxx/game-koi' }],
        sidebar: [
            { label: 'Getting started', slug: 'getting-started' },
            {
                label: 'Examples',
                items: [
                    { label: 'ROM player', slug: 'examples/rom-player' },
                    { label: 'Three.js scene', link: '/examples/threejs/' },
                ],
            },
        ],
		}), svelte()],
});