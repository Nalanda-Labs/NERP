// Background blur in calls: copies MediaPipe's WebAssembly files into static/ and fetches the
// selfie-segmentation model once, so both are served by NERP itself (the page's content
// security policy allows only 'self'). Without network access the model is skipped and blur
// stays unavailable; everything else builds.
import { cpSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
const wasmFrom = new URL('../node_modules/@mediapipe/tasks-vision/wasm/', import.meta.url);
const to = new URL('../static/mediapipe/', import.meta.url);
const MODEL = 'https://storage.googleapis.com/mediapipe-models/image_segmenter/selfie_segmenter/float16/latest/selfie_segmenter.tflite';
mkdirSync(new URL('wasm/', to), { recursive: true });
if (!existsSync(new URL('wasm/vision_wasm_internal.wasm', to))) cpSync(wasmFrom, new URL('wasm/', to), { recursive: true });
const model = new URL('selfie_segmenter.tflite', to);
if (!existsSync(model)) {
	try {
		const res = await fetch(MODEL);
		if (!res.ok) throw new Error(`HTTP ${res.status}`);
		writeFileSync(model, Buffer.from(await res.arrayBuffer()));
	} catch (e) {
		console.warn(`background blur: could not fetch the segmentation model (${e.message}); blur stays off`);
	}
}
