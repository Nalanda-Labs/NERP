<script lang="ts">
	// A published website page: the site's menu, the page's blocks, the footer. No sign-in.
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import SiteBlocks, { type Block } from '#lib/components/SiteBlocks.svelte';

	type Site = {
		name: string;
		color: string;
		font: string;
		footer: string | null;
		logo: string | null;
		menu: { title: string; path: string }[];
		analytics_id: string | null;
		cookie_banner: boolean;
		cookie_text: string | null;
	};
	let site = $state<Site | null>(null);
	let p = $state<{ title: string; description: string | null; blocks: Block[] } | null>(null);
	let missing = $state(false);
	const slug = $derived(page.params.slug ?? '');
	const path = $derived(page.params.path ?? '');
	let menuOpen = $state(false);
	// On the site's own domain addresses have no /s/<site> in front.
	const base = $derived(
		page.url.pathname === `/s/${slug}` || page.url.pathname.startsWith(`/s/${slug}/`)
			? `/s/${slug}`
			: ''
	);
	const href = (p: string) => (p ? `${base}/${p}` : base || '/');

	// Cookies: the visitor's choice, remembered on this device.
	const CONSENT = 'site.consent';
	let consent = $state<string | null>(null);
	try {
		consent = localStorage.getItem(CONSENT);
	} catch {
		/* private mode */
	}
	function choose(v: 'yes' | 'no') {
		consent = v;
		try {
			localStorage.setItem(CONSENT, v);
		} catch {
			/* not remembered */
		}
	}
	// Google Analytics, only on the site's own domain (never where NERP itself runs), and
	// only after consent when the site asks for it.
	let analyticsOn = false;
	$effect(() => {
		const id = site?.analytics_id;
		if (!id || base || analyticsOn || (site?.cookie_banner && consent !== 'yes')) return;
		analyticsOn = true;
		const w = window as unknown as { dataLayer: unknown[]; gtag: (...a: unknown[]) => void };
		w.dataLayer = w.dataLayer || [];
		w.gtag = function () {
			// eslint-disable-next-line prefer-rest-params
			w.dataLayer.push(arguments);
		};
		w.gtag('js', new Date());
		w.gtag('config', id);
		const tag = document.createElement('script');
		tag.async = true;
		tag.src = `https://www.googletagmanager.com/gtag/js?id=${encodeURIComponent(id)}`;
		document.head.appendChild(tag);
	});

	$effect(() => {
		const s = slug;
		api
			.get<Site>(`/public/sites/${encodeURIComponent(s)}`)
			.then((x) => (site = x))
			.catch(() => (missing = true));
	});
	$effect(() => {
		const s = slug,
			pth = path;
		p = null;
		menuOpen = false;
		api
			.get<{ title: string; description: string | null; blocks: Block[] }>(
				`/public/sites/${encodeURIComponent(s)}/page?path=${encodeURIComponent(pth)}`
			)
			.then((x) => ((p = x), (missing = false)))
			.catch(() => (missing = true));
	});
</script>

<svelte:head>
	<title
		>{p
			? path
				? `${p.title} · ${site?.name ?? ''}`
				: (site?.name ?? p.title)
			: (site?.name ?? '')}</title
	>
	{#if p?.description}<meta name="description" content={p.description} />{/if}
</svelte:head>

<div class="flex min-h-screen flex-col bg-white {site?.font === 'serif' ? 'font-serif' : ''}">
	{#if site}
		<header class="border-b border-slate-100">
			<div class="mx-auto flex h-16 max-w-5xl items-center gap-4 px-6">
				<a href={href('')} class="flex items-center gap-2 font-semibold text-slate-900"
					>{#if site.logo}<img src={site.logo} alt="" class="h-8 w-auto" />{/if}{site.name}</a
				>
				<nav class="ml-auto hidden gap-6 text-sm sm:flex" aria-label="Site">
					{#each site.menu as m (m.path)}<a
							href={href(m.path)}
							class={m.path === path ? 'font-semibold' : 'text-slate-600 hover:text-slate-900'}
							style={m.path === path ? `color:${site.color}` : ''}>{m.title}</a
						>{/each}
				</nav>
				<button
					class="ml-auto p-2 sm:hidden"
					aria-label="Menu"
					aria-expanded={menuOpen}
					onclick={() => (menuOpen = !menuOpen)}>☰</button
				>
			</div>
			{#if menuOpen}<nav class="border-t border-slate-100 px-6 py-2 sm:hidden">
					{#each site.menu as m (m.path)}<a href={href(m.path)} class="block py-2 text-slate-700"
							>{m.title}</a
						>{/each}
				</nav>{/if}
		</header>
	{/if}
	<main class="flex-1">
		{#if missing}
			<div class="mx-auto max-w-md px-6 py-24 text-center">
				<h1 class="text-2xl font-semibold text-slate-900">Page not found</h1>
				{#if site}<a
						class="mt-4 inline-block hover:underline"
						style="color:{site.color}"
						href={href('')}>Go to the home page</a
					>{/if}
			</div>
		{:else if p && site}
			<SiteBlocks blocks={p.blocks} color={site.color} />
		{/if}
	</main>
	{#if site?.cookie_banner && !consent}
		<div
			class="fixed inset-x-0 bottom-0 z-50 border-t border-slate-200 bg-white/95 p-4 shadow-lg backdrop-blur"
			role="dialog"
			aria-label="Cookies"
			data-cookie-banner
		>
			<div class="mx-auto flex max-w-5xl flex-wrap items-center gap-3 text-sm text-slate-700">
				<p class="flex-1">
					{site.cookie_text ?? 'We use cookies to see how the site is used and to improve it.'}
				</p>
				<button class="rounded-full border border-slate-300 px-4 py-2" onclick={() => choose('no')}
					>Only necessary</button
				>
				<button
					class="rounded-full px-4 py-2 font-semibold text-white"
					style="background:{site.color}"
					onclick={() => choose('yes')}>Accept</button
				>
			</div>
		</div>
	{/if}
	{#if site}
		<footer class="border-t border-slate-100">
			<div class="mx-auto max-w-5xl px-6 py-8 text-sm text-slate-500">
				{site.footer ?? `© ${new Date().getFullYear()} ${site.name}`}
			</div>
		</footer>
	{/if}
</div>
