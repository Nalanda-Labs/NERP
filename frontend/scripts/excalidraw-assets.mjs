// Copies the whiteboard's fonts (Excalidraw) into static/, so they're served by NERP itself
// rather than fetched from a CDN (the page's content security policy allows only 'self').
import { cpSync, existsSync, mkdirSync } from 'node:fs';
const from = new URL('../node_modules/@excalidraw/excalidraw/dist/prod/fonts/', import.meta.url);
const to = new URL('../static/excalidraw-assets/fonts/', import.meta.url);
if (!existsSync(to)) {
	mkdirSync(to, { recursive: true });
	cpSync(from, to, { recursive: true });
}
