// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

import svelte from '@astrojs/svelte';
import react from '@astrojs/react';
import preact from '@astrojs/preact';
import vue from '@astrojs/vue';

// readthedocs serves every version under its own path prefix — /en/latest/ for the
// default version, /en/<pr-number>/ for a pull request preview, /en/<tag>/ for a tagged
// one — so neither the origin nor the base is a constant that can be written down here.
// Getting this wrong is silent: the pages build and deploy, and every stylesheet, script
// and internal link 404s because it points at the server root rather than the prefix.
//
// READTHEDOCS_CANONICAL_URL is set during a readthedocs build and carries both halves.
// Without it — `just build-docs`, `just serve-docs`, the docs job in CI — the site is
// served from the root, which is what the local preview wants.
//
// DOCS_CANONICAL_URL is the same thing for any other host, and wins when both are set.
// The GitHub Pages workflow sets it from actions/configure-pages, since a project site
// lives under /<repo>/ and has exactly the same prefix problem.
const canonical = new URL(
    process.env.DOCS_CANONICAL_URL ??
        process.env.READTHEDOCS_CANONICAL_URL ??
        'https://game-koi.readthedocs.io/',
);
// Astro takes the base without a trailing slash; undefined leaves it at the root.
const base = canonical.pathname.replace(/\/+$/, '') || undefined;

// Astro and Starlight base-correct their own output — assets, page routes, the sidebar —
// but a link written by hand is emitted exactly as typed. So **write internal links
// relative** (`../examples/svelte/`), never root-absolute (`/examples/svelte/`):
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
        // Pixel art in the DMG's four greens. The favicon is the same image padded to a
        // square with its own background colour, so browsers do not stretch it.
        logo: { src: './src/assets/koi.png' },
        favicon: '/favicon.png',
        social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/KeeTraxx/game-koi' }],
        sidebar: [
            {
                label: 'Examples',
                items: [
                    { label: 'VanillaJS', slug: 'examples/vanilla-js' },
                    { label: 'Svelte', slug: 'examples/svelte' },
                    { label: 'React', slug: 'examples/react' },
                    { label: 'Preact', slug: 'examples/preact' },
                    { label: 'Vue.js', slug: 'examples/vue' },
                    { label: 'Three.js', link: '/examples/threejs/' },
                ],
            },
        ],
    }),
        svelte(),
        // React and Preact both claim .tsx files, so each is told which directory is
        // its own; without `include` Astro guesses per component and can pick wrong.
        react({ include: ['**/components/react/**'] }),
        preact({ include: ['**/components/preact/**'] }),
        vue(),
    ],
    // Pages that were renamed when the docs became example-first. Astro base-corrects
    // the *source* of a redirect but emits the destination exactly as written, so the
    // base has to be prepended by hand or the redirect lands on the server root.
    redirects: {
        '/getting-started': `${base ?? ''}/examples/vanilla-js/`,
        '/examples/rom-player': `${base ?? ''}/examples/svelte/`,
    },
});