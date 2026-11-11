<script lang="ts">
	// Rendered Markdown (HTML already cleaned by the server), made to work in the app:
	// pictures stored in NERP are fetched with the reader's sign-in, links to stored files
	// download, code blocks are highlighted, and headings get a link to copy.
	import { tick } from 'svelte';
	import { downloadFile, streamUrl } from '#lib/api.ts';
	import 'highlight.js/styles/github-dark.css';

	let { html, class: cls = '' }: { html: string; class?: string } = $props();
	let el: HTMLDivElement | undefined = $state();
	const FILE = /^\/api\/files\/([0-9a-f-]{36})\/content$/;
	let hljs: typeof import('highlight.js/lib/common').default | null = null;

	$effect(() => {
		html;
		tick().then(enhance);
	});

	async function enhance() {
		if (!el) return;
		// Pictures in NERP: a short-lived link the browser can load.
		for (const img of el.querySelectorAll<HTMLImageElement>('img')) {
			const src = img.getAttribute('src') ?? '';
			if (FILE.test(src) && !img.dataset.loaded) {
				img.dataset.loaded = '1';
				img.loading = 'lazy';
				streamUrl(src).then((u) => (img.src = u)).catch(() => (img.alt = `${img.alt} (can't be shown)`));
			}
		}
		// Code: highlighted by language (loaded the first time it is needed).
		const blocks = el.querySelectorAll<HTMLElement>('pre code');
		if (blocks.length) {
			hljs ??= (await import('highlight.js/lib/common')).default;
			for (const b of blocks) {
				if (b.dataset.highlighted) continue;
				const lang = [...b.classList].find((c) => c.startsWith('language-'))?.slice(9);
				if (lang && !hljs.getLanguage(lang)) continue;
				hljs.highlightElement(b);
			}
		}
		// Headings: a link to this place on the page.
		for (const h of el.querySelectorAll<HTMLElement>('h1[id], h2[id], h3[id], h4[id]')) {
			if (h.querySelector('.anchor')) continue;
			const a = document.createElement('a');
			a.href = `#${h.id}`;
			a.className = 'anchor';
			a.textContent = '#';
			a.setAttribute('aria-label', `Link to “${h.textContent}”`);
			h.append(a);
		}
	}

	function click(e: MouseEvent) {
		const a = (e.target as HTMLElement).closest('a');
		if (!a) return;
		const href = a.getAttribute('href') ?? '';
		const m = href.match(FILE);
		if (m) {
			e.preventDefault();
			downloadFile(m[1], a.textContent?.trim() || 'file');
			return;
		}
		if (a.classList.contains('anchor')) {
			e.preventDefault();
			const url = `${location.origin}${location.pathname}${href}`;
			navigator.clipboard?.writeText(url).catch(() => {});
			history.replaceState(null, '', href);
			document.getElementById(href.slice(1))?.scrollIntoView({ behavior: 'smooth' });
		}
	}
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="prose-kb {cls}" bind:this={el} onclick={click} data-markdown>{@html html}</div>
