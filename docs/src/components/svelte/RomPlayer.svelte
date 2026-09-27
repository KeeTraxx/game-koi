<script lang="ts">
	import { onDestroy } from 'svelte';
	// A type-only import is erased at compile time, so it is safe at module scope.
	import type { GameKoi } from 'game-koi';

	let canvas: HTMLCanvasElement;
	let status = $state('ready — choose a ROM (.gb)');
	// Not $state: nothing in the markup reads it, so there is nothing to re-render.
	let koi: GameKoi | null = null;

	async function handleFile(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;

		try {
			// Imported here, not at module scope: `game-koi` touches AudioContext,
			// wasm and the DOM as soon as it is evaluated, so it must never run during
			// server-side rendering. Deferring it to the handler keeps it in the browser.
			const { GameKoi } = await import('game-koi');
			const rom = new Uint8Array(await file.arrayBuffer());
			if (koi) {
				// A second create() would build a second emulator and AudioContext.
				koi.loadRom(rom);
			} else {
				koi = await GameKoi.create({ canvas, rom });
			}
			status = `running ${file.name}`;
		} catch (err) {
			// A bad header or an unsupported mapper arrives here as a thrown error.
			status = `error: ${err}`;
		}
	}

	// Stops the frame loop, removes the key listeners and closes the AudioContext.
	onDestroy(() => koi?.dispose());
</script>

<div class="rom-player">
	<canvas bind:this={canvas} width="160" height="144"></canvas>
	<p class="status">{status}</p>
	<input type="file" accept=".gb" onchange={handleFile} />
</div>

<style>
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
