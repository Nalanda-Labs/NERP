// Loads Excalidraw on demand, with its fonts served by NERP itself (the page's content
// security policy allows only 'self'); the asset path must be set before the first import.
//
// Excalidraw always lists a CDN copy (esm.sh) after our own in each font's sources; the
// browser reports those as policy violations even though ours load. Fonts are given only
// their same-site sources.

let patched = false;

function sameSiteFonts() {
	if (patched || typeof window === 'undefined') return;
	patched = true;
	const Native = window.FontFace;
	const keep = (src: string) => src.split(/,\s*(?=url\()/).filter((part) => !/url\(\s*["']?https?:\/\//.test(part) || part.includes(location.origin)).join(', ');
	class SameSiteFontFace extends Native {
		constructor(family: string, source: string | BufferSource, descriptors?: FontFaceDescriptors) {
			super(family, typeof source === 'string' ? keep(source) || source : source, descriptors);
		}
	}
	window.FontFace = SameSiteFontFace as typeof FontFace;
}

export async function loadExcalidraw() {
	(window as unknown as { EXCALIDRAW_ASSET_PATH: string }).EXCALIDRAW_ASSET_PATH = `${location.origin}/excalidraw-assets/`;
	sameSiteFonts();
	return import('@excalidraw/excalidraw');
}
