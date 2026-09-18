<script lang="ts">
	let canvas: HTMLCanvasElement;
	let status = $state('ready — choose a ROM (.gb)');
	let koi: import('game-koi').GameKoi | null = null;

	async function handleFile(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;

		try {
			// Imported here, not at module scope: `game-koi` touches AudioContext,
			// wasm and the DOM as soon as it is evaluated, and Vite walks this
			// module's static imports during `astro build`'s SSG pass even under
			// `client:only`. Deferring it to the handler keeps it in the browser.
			const { GameKoi } = await import('game-koi');
			const rom = new Uint8Array(await file.arrayBuffer());
			if (koi) {
				koi.loadRom(rom);
			} else {
				koi = await GameKoi.create({ canvas, rom });
			}
			status = `running ${file.name}`;
		} catch (err) {
			// A bad header or an unsupported mapper arrives here as a thrown JsError.
			status = `error: ${err}`;
			koi = null;
		}
	}
</script>

<div class="rom-player">
	<canvas bind:this={canvas} width="160" height="144"></canvas>
	<p class="status">{status}</p>
	<label>
		<input type="file" accept=".gb" onchange={handleFile} />
	</label>
	<p class="note">
		No ROM ships with this page — bring your own <code>.gb</code> file. Nothing
		you pick is uploaded anywhere; it's read straight into the emulator running
		in your browser tab.
	</p>
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

	.note {
		font-size: var(--sl-text-xs);
		color: var(--sl-color-gray-3);
		max-width: 32rem;
	}
</style>
