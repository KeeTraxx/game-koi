import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { GameKoi } from 'game-koi';

const sceneCanvas = document.getElementById('scene') as HTMLCanvasElement;
const gbCanvas = document.getElementById('gb') as HTMLCanvasElement;
const romInput = document.getElementById('rom') as HTMLInputElement;
const statusEl = document.getElementById('status') as HTMLElement;

// An unlit DMG screen is pale green, not black — fill it so the model looks like
// hardware that is switched on but has no cartridge, rather than a hole.
const gbCtx = gbCanvas.getContext('2d')!;
gbCtx.fillStyle = '#e0f8d0';
gbCtx.fillRect(0, 0, gbCanvas.width, gbCanvas.height);

// The whole trick: a texture backed by the canvas the emulator draws into.
const screenTexture = new THREE.CanvasTexture(gbCanvas);
// Nearest, and no mipmaps, or a 160x144 image stretched over a quad turns to mush —
// the same reason the plain canvas demos set `image-rendering: pixelated`.
screenTexture.magFilter = THREE.NearestFilter;
screenTexture.minFilter = THREE.NearestFilter;
screenTexture.generateMipmaps = false;
screenTexture.colorSpace = THREE.SRGBColorSpace;

const scene = new THREE.Scene();
scene.background = new THREE.Color(0x14181c);

const camera = new THREE.PerspectiveCamera(45, 4 / 3, 0.1, 100);
camera.position.set(0, 0.6, 4.2);

const renderer = new THREE.WebGLRenderer({ canvas: sceneCanvas, antialias: true });
renderer.setPixelRatio(Math.min(devicePixelRatio, 2));

scene.add(new THREE.AmbientLight(0xffffff, 1.2));
const key = new THREE.DirectionalLight(0xffffff, 2);
key.position.set(2, 3, 4);
scene.add(key);

const shell = new THREE.MeshStandardMaterial({ color: 0x8b8f93, roughness: 0.75 });
// BoxGeometry's material slots run +X, -X, +Y, -Y, +Z, -Z — so index 4 is the face
// pointing at the camera. MeshBasicMaterial ignores lights, which is what keeps the
// emulator's own colours intact instead of letting the key light tint them.
const screen = new THREE.MeshBasicMaterial({ map: screenTexture });
// 2.0 x 1.8 is the DMG's 10:9, so the picture lands on the face undistorted.
const body = new THREE.Mesh(
	new THREE.BoxGeometry(2.0, 1.8, 0.25),
	[shell, shell, shell, shell, screen, shell],
);
scene.add(body);

const controls = new OrbitControls(camera, sceneCanvas);
controls.enableDamping = true;
controls.autoRotate = true;
controls.autoRotateSpeed = 1.2;
controls.minDistance = 2.5;
controls.maxDistance = 8;

function resize() {
	const { clientWidth: w, clientHeight: h } = sceneCanvas;
	if (w === 0 || h === 0) return;
	renderer.setSize(w, h, false);
	camera.aspect = w / h;
	camera.updateProjectionMatrix();
}
new ResizeObserver(resize).observe(sceneCanvas);
resize();

renderer.setAnimationLoop(() => {
	// GameKoi runs its own requestAnimationFrame loop and repaints the canvas on its
	// own schedule, so there is no callback to hook — re-upload every frame and let
	// the two clocks stay independent. It is a 160x144 texture; the cost is noise.
	screenTexture.needsUpdate = true;
	controls.update();
	renderer.render(scene, camera);
});

let koi: GameKoi | null = null;

romInput.addEventListener('change', async () => {
	const file = romInput.files?.[0];
	if (!file) return;

	try {
		const rom = new Uint8Array(await file.arrayBuffer());
		if (koi) {
			koi.loadRom(rom);
		} else {
			// The change event is the user gesture an AudioContext needs.
			koi = await GameKoi.create({
				canvas: gbCanvas,
				rom,
				// The stats panel positions itself from its canvas's bounding rect,
				// which is meaningless when that canvas is hidden.
				overlayKey: null,
			});
		}
		statusEl.textContent = `running ${file.name}`;
	} catch (err) {
		statusEl.textContent = `error: ${err}`;
		koi = null;
	}
});
