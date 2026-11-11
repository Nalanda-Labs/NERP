<script lang="ts">
	// Public: a file or a folder shared with a link from the drive, with its password if it
	// has one. In a folder, visitors open subfolders and download files.
	import { page } from '$app/state';
	import { formatBytes } from '#lib/format.ts';

	type Item = { id: string; name: string; kind: 'file' | 'folder'; size_bytes: number; mime_type: string | null };
	type Info = { kind: 'file' | 'folder'; name: string; size_bytes?: number; root?: string; path?: { id: string; name: string }[]; items?: Item[] };
	const token = $derived(page.params.token ?? '');
	let info = $state<Info | null>(null);
	let missing = $state(false);
	let needPassword = $state(false);
	let password = $state('');
	let error = $state('');
	let at = $state<string | null>(null);

	const base = $derived(`/api/public/drive/${encodeURIComponent(token)}`);
	const headers = () => (password ? { 'X-Link-Password': password } : undefined);
	async function load(item: string | null = at) {
		error = '';
		const r = await fetch(`${base}${item ? `?item=${item}` : ''}`, { headers: headers() });
		if (r.status === 401) {
			needPassword = true;
			if (password) error = (await r.json().catch(() => ({})))?.error?.message ?? 'Wrong password';
			return;
		}
		if (!r.ok) return void (missing = true);
		needPassword = false;
		info = await r.json();
		at = item;
	}
	$effect(() => {
		token;
		load(null);
	});
	async function download(item: string | null, name: string) {
		const url = `${base}/download${item ? `?item=${item}` : ''}`;
		if (!password) return void (location.href = url);
		// A password travels in a header, so the file comes through here.
		const r = await fetch(url, { headers: headers() });
		if (!r.ok) return void (error = 'The download did not work');
		const a = document.createElement('a');
		a.href = URL.createObjectURL(await r.blob());
		a.download = name;
		a.click();
		setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
	}
</script>

<svelte:head><title>{info?.name ?? 'Shared file'}</title><meta name="robots" content="noindex" /></svelte:head>

<main class="flex min-h-screen items-center justify-center bg-slate-50 p-4">
	<div class="w-full {info?.kind === 'folder' ? 'max-w-2xl' : 'max-w-md text-center'} rounded-3xl bg-white p-8 shadow-g1">
		{#if missing}
			<h1 class="text-xl font-semibold text-slate-900">This link doesn't work</h1>
			<p class="mt-2 text-sm text-slate-600">It may have expired or been removed.</p>
		{:else if needPassword}
			<form class="space-y-3 text-left" onsubmit={(e) => (e.preventDefault(), load(null))}>
				<h1 class="text-lg font-semibold text-slate-900">This link needs a password</h1>
				<input class="input" type="password" aria-label="Password" required bind:value={password} />
				{#if error}<p class="text-sm text-red-600" role="alert">{error}</p>{/if}
				<button class="btn btn-primary w-full justify-center">Open</button>
			</form>
		{:else if info?.kind === 'folder'}
			<nav class="mb-1 flex flex-wrap items-center gap-1 text-sm text-slate-500" aria-label="Folders">
				{#each info.path ?? [] as p, i (p.id)}
					{#if i > 0}<span>/</span>{/if}
					<button class="hover:underline {i === (info.path?.length ?? 0) - 1 ? 'font-semibold text-slate-900' : ''}" onclick={() => load(i === 0 ? null : p.id)}>{p.name}</button>
				{/each}
			</nav>
			<ul class="mt-3 divide-y divide-slate-100" data-folder-items>
				{#each info.items ?? [] as it (it.id)}
					<li class="flex items-center gap-3 py-2 text-sm">
						<span class="w-5 text-center">{it.kind === 'folder' ? '📁' : '📄'}</span>
						{#if it.kind === 'folder'}
							<button class="flex-1 truncate text-left font-medium text-slate-800 hover:underline" onclick={() => load(it.id)}>{it.name}</button>
						{:else}
							<span class="flex-1 truncate text-slate-800" data-file-name>{it.name}</span>
							<span class="text-xs text-slate-400">{formatBytes(it.size_bytes)}</span>
							<button class="btn btn-secondary px-3 py-1 text-xs" onclick={() => download(it.id, it.name)}>Download</button>
						{/if}
					</li>
				{:else}<li class="py-3 text-sm text-slate-400">This folder is empty.</li>{/each}
			</ul>
			{#if error}<p class="mt-2 text-sm text-red-600" role="alert">{error}</p>{/if}
		{:else if info}
			<h1 class="text-xl font-semibold break-words text-slate-900" data-file-name>{info.name}</h1>
			<p class="mt-1 text-sm text-slate-500">{formatBytes(info.size_bytes ?? 0)}</p>
			<button class="btn btn-primary mt-6" onclick={() => download(null, info!.name)}>Download</button>
			{#if error}<p class="mt-2 text-sm text-red-600" role="alert">{error}</p>{/if}
		{:else}
			<p class="text-sm text-slate-400">Loading…</p>
		{/if}
	</div>
</main>
