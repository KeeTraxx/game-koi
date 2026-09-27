import { useEffect, useRef, useState, type ChangeEvent } from 'react';
// A type-only import is erased at compile time, so it is safe at module scope.
import type { GameKoi } from 'game-koi';
import '../rom-player.css';

export default function RomPlayer() {
	const canvasRef = useRef<HTMLCanvasElement>(null);
	// A ref, not state: the emulator is never rendered, and swapping it must not
	// trigger a re-render.
	const koiRef = useRef<GameKoi | null>(null);
	const [status, setStatus] = useState('ready — choose a ROM (.gb)');

	// Stops the frame loop, removes the key listeners and closes the AudioContext
	// when the component unmounts.
	useEffect(() => {
		return () => {
			koiRef.current?.dispose();
			koiRef.current = null;
		};
	}, []);

	async function handleFile(event: ChangeEvent<HTMLInputElement>) {
		const file = event.currentTarget.files?.[0];
		if (!file || !canvasRef.current) return;

		try {
			// Imported here, not at module scope: `game-koi` touches AudioContext,
			// wasm and the DOM as soon as it is evaluated, so it must never run during
			// server-side rendering (Next.js, Remix, Astro...).
			const { GameKoi } = await import('game-koi');
			const rom = new Uint8Array(await file.arrayBuffer());
			if (koiRef.current) {
				// A second create() would build a second emulator and AudioContext.
				koiRef.current.loadRom(rom);
			} else {
				koiRef.current = await GameKoi.create({ canvas: canvasRef.current, rom });
			}
			setStatus(`running ${file.name}`);
		} catch (err) {
			// A bad header or an unsupported mapper arrives here as a thrown error.
			setStatus(`error: ${err}`);
		}
	}

	return (
		<div className="rom-player">
			<canvas ref={canvasRef} width={160} height={144} />
			<p className="status">{status}</p>
			<input type="file" accept=".gb" onChange={handleFile} />
		</div>
	);
}
