import { GameKoi } from 'game-koi';

const canvas = document.querySelector<HTMLCanvasElement>('#screen')!;
const romInput = document.querySelector<HTMLInputElement>('#rom')!;
const statusEl = document.querySelector<HTMLElement>('#status')!;

let koi: GameKoi | null = null;

romInput.addEventListener('change', async () => {
	const file = romInput.files?.[0];
	if (!file) return;

	try {
		const rom = new Uint8Array(await file.arrayBuffer());
		if (koi) {
			// A second create() would build a second emulator and AudioContext.
			koi.loadRom(rom);
		} else {
			// The change event is the user gesture an AudioContext needs to start.
			koi = await GameKoi.create({ canvas, rom });
		}
		statusEl.textContent = `running ${file.name}`;
	} catch (err) {
		// A bad header or an unsupported mapper arrives here as a thrown error.
		statusEl.textContent = `error: ${err}`;
	}
});
