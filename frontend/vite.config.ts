import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// NERP is a single-page app: every route renders in the browser and the
			// backend serves build/index.html for any path it does not know.
			// precompress writes .br and .gz copies that the backend serves as is.
			adapter: adapter({ fallback: 'index.html', precompress: true })
		})
	],
	server: {
		proxy: {
			'/api': { target: process.env.NERP_API ?? 'http://127.0.0.1:8080', changeOrigin: false, ws: true }
		}
	}
});
