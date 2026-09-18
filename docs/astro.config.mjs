// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

import svelte from '@astrojs/svelte';

// https://astro.build/config
export default defineConfig({
    // readthedocs.com serves the built dist/ here; Astro needs it for the sitemap
    // and for canonical URLs.
    site: 'https://game-koi.readthedocs.io',
    integrations: [starlight({
        title: 'game-koi',
        social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/KeeTraxx/game-koi' }],
        sidebar: [
            { label: 'Getting started', slug: 'getting-started' },
            {
                label: 'Examples',
                items: [
                    { label: 'ROM player', slug: 'examples/rom-player' },
                    { label: 'Three.js island', link: '/examples/threejs/' },
                ],
            },
        ],
		}), svelte()],
});