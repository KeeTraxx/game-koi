import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { GLTFLoader } from 'three/examples/jsm/loaders/GLTFLoader.js';
import { GameKoi } from 'game-koi';
// `?url` has Vite copy the model into the build and hand back its final URL, base path
// included — so it resolves under readthedocs' /en/<version>/ prefix, where a
// hand-written '/models/gb_model.glb' would point at the server root and 404.
import modelUrl from '../assets/gb_model.glb?url';

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
// glTF puts the UV origin top-left, so Blender's exporter flips V and GLTFLoader sets
// flipY = false on every texture it loads. This one does not come from the loader and
// would keep three's default of true — and the game would play upside down.
screenTexture.flipY = false;

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

// MeshBasicMaterial ignores lights, which is what keeps the emulator's own colours
// intact instead of letting the key light tint them; toneMapped: false does the same
// for the renderer's output transform.
const screen = new THREE.MeshBasicMaterial({ map: screenTexture, toneMapped: false });

new GLTFLoader().load(
	modelUrl,
	(gltf) => {
		const model = gltf.scene;
		// The rest of the model keeps the materials it was given in Blender; only the
		// "Display" quad is swapped for the live screen. Its UVs span 0..1 over the one
		// face, so the whole 160x144 frame lands on it with no further mapping.
		const display = model.getObjectByName('Display');
		if (!(display instanceof THREE.Mesh)) throw new Error('model has no "Display" mesh');
		display.material = screen;

		// Modelled lying on its back, screen facing Blender's +Z — which the exporter's
		// Z-up to Y-up conversion turns into three's +Y. A quarter turn about X stands it
		// up facing the camera, with the cartridge slot on top.
		model.rotation.x = Math.PI / 2;

		// Blender works in metres and this is life-size (14.8 cm tall), while the camera
		// and orbit limits below are set up for a body about two units tall. Scale to
		// fit and centre on the origin, which is what OrbitControls circles.
		const box = new THREE.Box3().setFromObject(model);
		const size = box.getSize(new THREE.Vector3());
		model.scale.setScalar(2.2 / size.y);
		box.setFromObject(model);
		model.position.sub(box.getCenter(new THREE.Vector3()));

		scene.add(model);
	},
	undefined,
	(err) => {
		statusEl.textContent = `error loading model: ${err}`;
	},
);

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
