// Background blur for calls: MediaPipe's selfie segmenter finds the person in each camera
// frame; the frame is drawn blurred and the person sharp on top, and that canvas becomes the
// video track sent to the call. Runs in the browser (nothing leaves it); the model and the
// WebAssembly files are served by NERP (scripts/mediapipe-assets.mjs).
import type { ImageSegmenter } from '@mediapipe/tasks-vision';

let segmenter: Promise<ImageSegmenter> | null = null;

function load(): Promise<ImageSegmenter> {
	segmenter ??= (async () => {
		const { FilesetResolver, ImageSegmenter } = await import('@mediapipe/tasks-vision');
		const files = await FilesetResolver.forVisionTasks('/mediapipe/wasm');
		const make = (delegate: 'GPU' | 'CPU') =>
			ImageSegmenter.createFromOptions(files, {
				baseOptions: { modelAssetPath: '/mediapipe/selfie_segmenter.tflite', delegate },
				runningMode: 'VIDEO',
				outputCategoryMask: false,
				outputConfidenceMasks: true
			});
		// The graphics card when there is one, else the processor.
		return make('GPU').catch(() => make('CPU'));
	})();
	segmenter.catch(() => (segmenter = null));
	return segmenter;
}

export type Blurred = { track: MediaStreamTrack; stop: () => void };

/** A blurred-background copy of a camera track (the original keeps running). */
export async function blurTrack(input: MediaStreamTrack, strength = 14): Promise<Blurred> {
	const seg = await load();
	const video = document.createElement('video');
	video.muted = true;
	video.playsInline = true;
	video.srcObject = new MediaStream([input]);
	await video.play();
	const w = video.videoWidth || input.getSettings().width || 640;
	const h = video.videoHeight || input.getSettings().height || 480;
	const out = document.createElement('canvas');
	const person = document.createElement('canvas');
	const mask = document.createElement('canvas');
	for (const c of [out, person, mask]) {
		c.width = w;
		c.height = h;
	}
	const ctx = out.getContext('2d')!;
	const pctx = person.getContext('2d')!;
	const mctx = mask.getContext('2d')!;
	const alpha = mctx.createImageData(w, h);
	let running = true;
	let last = -1;

	const frame = () => {
		if (!running) return;
		if (video.currentTime !== last && video.readyState >= 2) {
			last = video.currentTime;
			const res = seg.segmentForVideo(video, performance.now());
			const conf = res.confidenceMasks?.[0];
			if (conf) {
				const m = conf.getAsFloat32Array();
				// The mask may be smaller than the frame: scale it when drawing.
				if (conf.width !== mask.width || conf.height !== mask.height) {
					mask.width = conf.width;
					mask.height = conf.height;
				}
				const img = conf.width === w && conf.height === h ? alpha : mctx.createImageData(conf.width, conf.height);
				for (let i = 0; i < m.length; i++) img.data[i * 4 + 3] = Math.min(255, m[i] * 255);
				mctx.putImageData(img, 0, 0);
			}
			res.close();
			// Background: the whole frame, blurred.
			ctx.globalCompositeOperation = 'copy';
			ctx.filter = `blur(${strength}px)`;
			ctx.drawImage(video, 0, 0, w, h);
			ctx.filter = 'none';
			// The person, sharp, cut out by the mask.
			pctx.globalCompositeOperation = 'copy';
			pctx.drawImage(mask, 0, 0, w, h);
			pctx.globalCompositeOperation = 'source-in';
			pctx.drawImage(video, 0, 0, w, h);
			ctx.globalCompositeOperation = 'source-over';
			ctx.drawImage(person, 0, 0);
		}
		if ('requestVideoFrameCallback' in video) video.requestVideoFrameCallback(frame);
		else requestAnimationFrame(frame);
	};
	frame();
	const track = out.captureStream(30).getVideoTracks()[0];
	return {
		track,
		stop: () => {
			running = false;
			track.stop();
			video.pause();
			video.srcObject = null;
		}
	};
}
