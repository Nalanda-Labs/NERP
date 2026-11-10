<script lang="ts">
	// A published website page: the site's menu, the page's blocks, the footer. No sign-in.
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import SiteBlocks, { type Block } from '#lib/components/SiteBlocks.svelte';

	type Site = { name: string; color: string; font: string; footer: string | null; logo: string | null; menu: { title: string; path: string }[] };
	let site = $state<Site | null>(null);
	let p = $state<{ title: string; description: string | null; blocks: Block[] } | null>(null);
	let missing = $state(false);
	const slug = $derived(page.params.slug ?? '');
	const path = $derived(page.params.path ?? '');
	let menuOpen = $state(false);

	$effect(() => {
		const s = slug;
		api.get<Site>(`/public/sites/${encodeURIComponent(s)}`).then((x) => (site = x)).catch(() => (missing = true));
	});
	$effect(() => {
		const s = slug, pth = path;
		p = null;
		menuOpen = false;
		api.get<{ title: string; description: string | null; blocks: Block[] }>(`/public/sites/${encodeURIComponent(s)}/page?path=${encodeURIComponent(pth)}`)
			.then((x) => ((p = x), (missing = false)))
			.catch(() => (missing = true));
	});
</script>

<svelte:head>
	<title>{p ? (path ? `${p.title} · ${site?.name ?? ''}` : (site?.name ?? p.title)) : (site?.name ?? '')}</title>
	{#if p?.description}<meta name="description" content={p.description} />{/if}
</svelte:head>

<div class="flex min-h-screen flex-col bg-white {site?.font === 'serif' ? 'font-serif' : ''}">
	{#if site}
		<header class="border-b border-slate-100">
			<div class="mx-auto flex h-16 max-w-5xl items-center gap-4 px-6">
				<a href="/s/{slug}" class="flex items-center gap-2 font-semibold text-slate-900">{#if site.logo}<img src={site.logo} alt="" class="h-8 w-auto" />{/if}{site.name}</a>
				<nav class="ml-auto hidden gap-6 text-sm sm:flex" aria-label="Site">
					{#each site.menu as m (m.path)}<a href="/s/{slug}{m.path ? `/${m.path}` : ''}" class={m.path === path ? 'font-semibold' : 'text-slate-600 hover:text-slate-900'} style={m.path === path ? `color:${site.color}` : ''}>{m.title}</a>{/each}
				</nav>
				<button class="ml-auto p-2 sm:hidden" aria-label="Menu" aria-expanded={menuOpen} onclick={() => (menuOpen = !menuOpen)}>☰</button>
			</div>
			{#if menuOpen}<nav class="border-t border-slate-100 px-6 py-2 sm:hidden">{#each site.menu as m (m.path)}<a href="/s/{slug}{m.path ? `/${m.path}` : ''}" class="block py-2 text-slate-700">{m.title}</a>{/each}</nav>{/if}
		</header>
	{/if}
	<main class="flex-1">
		{#if missing}
			<div class="mx-auto max-w-md px-6 py-24 text-center"><h1 class="text-2xl font-semibold text-slate-900">Page not found</h1>{#if site}<a class="mt-4 inline-block hover:underline" style="color:{site.color}" href="/s/{slug}">Go to the home page</a>{/if}</div>
		{:else if p && site}
			<SiteBlocks blocks={p.blocks} color={site.color} />
		{/if}
	</main>
	{#if site}
		<footer class="border-t border-slate-100"><div class="mx-auto max-w-5xl px-6 py-8 text-sm text-slate-500">{site.footer ?? `© ${new Date().getFullYear()} ${site.name}`}</div></footer>
	{/if}
</div>
