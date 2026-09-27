<script setup lang="ts">
import { onBeforeUnmount, ref, useTemplateRef } from 'vue';
// A type-only import is erased at compile time, so it is safe at module scope.
import type { GameKoi } from 'game-koi';

const canvas = useTemplateRef<HTMLCanvasElement>('canvas');
const status = ref('ready — choose a ROM (.gb)');
// A plain variable, not ref(): the template never reads it, and wrapping the emulator
// in a reactive proxy would buy nothing.
let koi: GameKoi | null = null;

async function handleFile(event: Event) {
	const file = (event.target as HTMLInputElement).files?.[0];
	if (!file || !canvas.value) return;

	try {
		// Imported here, not at module scope: `game-koi` touches AudioContext, wasm
		// and the DOM as soon as it is evaluated, so it must never run during
		// server-side rendering (Nuxt, Astro...).
		const { GameKoi } = await import('game-koi');
		const rom = new Uint8Array(await file.arrayBuffer());
		if (koi) {
			// A second create() would build a second emulator and AudioContext.
			koi.loadRom(rom);
		} else {
			koi = await GameKoi.create({ canvas: canvas.value, rom });
		}
		status.value = `running ${file.name}`;
	} catch (err) {
		// A bad header or an unsupported mapper arrives here as a thrown error.
		status.value = `error: ${err}`;
	}
}

// Stops the frame loop, removes the key listeners and closes the AudioContext.
onBeforeUnmount(() => koi?.dispose());
</script>

<template>
	<div class="rom-player">
		<canvas ref="canvas" width="160" height="144"></canvas>
		<p class="status">{{ status }}</p>
		<input type="file" accept=".gb" @change="handleFile" />
	</div>
</template>

<style scoped>
.rom-player {
	display: flex;
	flex-direction: column;
	gap: 0.75rem;
	align-items: flex-start;
}

canvas {
	width: 320px;
	height: 288px;
	image-rendering: pixelated;
	background: black;
	border: 1px solid var(--sl-color-gray-5);
}

.status {
	font-family: var(--__sl-font-mono, monospace);
	margin: 0;
}
</style>
