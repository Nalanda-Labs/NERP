<script lang="ts">
	// The Excalidraw editor (a React component) inside a Svelte page. Loaded on demand: it is
	// large, and only board pages need it.
	/* eslint-disable @typescript-eslint/no-explicit-any */
	import { onDestroy, onMount } from 'svelte';
	import type { El } from '#lib/boards.svelte.ts';
	import { loadExcalidraw } from '#lib/excalidraw.ts';

	let {
		elements = [],
		appState = {},
		readOnly = false,
		name = 'board',
		onready,
		onchange
	}: {
		elements?: El[];
		appState?: Record<string, unknown>;
		readOnly?: boolean;
		name?: string;
		onready?: (api: any, lib: any) => void;
		onchange?: (elements: readonly El[], appState: any, files: any) => void;
	} = $props();

	let host: HTMLDivElement;
	let root: any = null;
	let failed = $state('');

	onMount(async () => {
		try {
			const [React, ReactDOM, lib] = await Promise.all([import('react'), import('react-dom/client'), loadExcalidraw()]);
			await import('@excalidraw/excalidraw/index.css');
			root = ReactDOM.createRoot(host);
			root.render(
				React.createElement(lib.Excalidraw, {
					initialData: { elements: elements as any, appState: { ...appState, viewModeEnabled: readOnly } as any, scrollToContent: true },
					viewModeEnabled: readOnly,
					name,
					UIOptions: { canvasActions: { loadScene: !readOnly, saveToActiveFile: false } },
					excalidrawAPI: (api: any) => onready?.(api, lib),
					onChange: (els: readonly El[], st: any, files: any) => onchange?.(els, st, files)
				})
			);
		} catch (e: any) {
			failed = e?.message ?? 'The whiteboard could not be loaded';
		}
	});
	onDestroy(() => root?.unmount());
</script>

<div class="relative size-full" data-excalidraw>
	<div bind:this={host} class="absolute inset-0"></div>
	{#if failed}<p class="absolute inset-0 flex items-center justify-center text-sm text-red-600">{failed}</p>{/if}
</div>
