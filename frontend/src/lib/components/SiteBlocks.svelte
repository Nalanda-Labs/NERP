<script module lang="ts">
	export type Block = {
		kind: 'hero' | 'text' | 'features' | 'image' | 'gallery' | 'cta' | 'contact' | 'form';
		heading?: string;
		text?: string;
		button_label?: string;
		button_link?: string;
		image?: string;
		image_url?: string | null;
		images?: string[];
		image_urls?: (string | null)[];
		markdown?: string;
		html?: string;
		items?: { title: string | null; text: string | null }[];
		caption?: string;
		address?: string;
		phone?: string;
		email?: string;
		hours?: string;
		form_key?: string;
	};
</script>

<script lang="ts">
	// A website page's blocks, as visitors see them (also the editor's preview).
	import { onMount } from 'svelte';

	let { blocks, color = '#1a73e8', src = null }: { blocks: Block[]; color?: string; src?: ((id: string) => string | null) | null } = $props();
	const img = (id: string | undefined, url: string | null | undefined) => (src && id ? src(id) : url) ?? null;

	// Web forms size themselves.
	let heights = $state<Record<string, number>>({});
	onMount(() => {
		const on = (e: MessageEvent) => {
			if (e.data?.nerpForm && e.data.height) heights[e.data.nerpForm] = e.data.height;
		};
		window.addEventListener('message', on);
		return () => window.removeEventListener('message', on);
	});
</script>

{#each blocks as b, i (i)}
	{#if b.kind === 'hero'}
		<section class="relative overflow-hidden" style="background:{color}" data-block="hero">
			{#if img(b.image, b.image_url)}<img src={img(b.image, b.image_url)} alt="" class="absolute inset-0 size-full object-cover opacity-30" />{/if}
			<div class="relative mx-auto max-w-5xl px-6 py-20 text-white sm:py-28">
				{#if b.heading}<h1 class="max-w-3xl text-4xl font-bold tracking-tight sm:text-5xl">{b.heading}</h1>{/if}
				{#if b.text}<p class="mt-4 max-w-2xl text-lg opacity-90">{b.text}</p>{/if}
				{#if b.button_label && b.button_link}<a href={b.button_link} class="mt-8 inline-block rounded-full bg-white px-6 py-3 font-semibold" style="color:{color}">{b.button_label}</a>{/if}
			</div>
		</section>
	{:else if b.kind === 'text'}
		<section class="mx-auto max-w-3xl px-6 py-10" data-block="text"><div class="prose-kb">{@html b.html ?? ''}</div></section>
	{:else if b.kind === 'features'}
		<section class="mx-auto max-w-5xl px-6 py-12" data-block="features">
			{#if b.heading}<h2 class="mb-8 text-center text-3xl font-bold text-slate-900">{b.heading}</h2>{/if}
			<div class="grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
				{#each b.items ?? [] as f, j (j)}
					<div class="rounded-2xl border border-slate-200 p-6"><span class="mb-3 block h-1 w-10 rounded-full" style="background:{color}"></span><p class="font-semibold text-slate-900">{f.title ?? ''}</p><p class="mt-2 text-slate-600">{f.text ?? ''}</p></div>
				{/each}
			</div>
		</section>
	{:else if b.kind === 'image'}
		<figure class="mx-auto max-w-4xl px-6 py-8" data-block="image">
			{#if img(b.image, b.image_url)}<img src={img(b.image, b.image_url)} alt={b.caption ?? ''} class="w-full rounded-2xl" />{/if}
			{#if b.caption}<figcaption class="mt-2 text-center text-sm text-slate-500">{b.caption}</figcaption>{/if}
		</figure>
	{:else if b.kind === 'gallery'}
		<section class="mx-auto grid max-w-5xl grid-cols-2 gap-3 px-6 py-8 sm:grid-cols-3" data-block="gallery">
			{#each b.images ?? [] as id, j (j)}{#if img(id, b.image_urls?.[j])}<img src={img(id, b.image_urls?.[j])} alt="" class="aspect-square w-full rounded-xl object-cover" />{/if}{/each}
		</section>
	{:else if b.kind === 'cta'}
		<section class="mx-auto max-w-5xl px-6 py-10" data-block="cta">
			<div class="rounded-3xl px-8 py-12 text-center text-white" style="background:{color}">
				{#if b.heading}<h2 class="text-3xl font-bold">{b.heading}</h2>{/if}
				{#if b.text}<p class="mx-auto mt-3 max-w-2xl opacity-90">{b.text}</p>{/if}
				{#if b.button_label && b.button_link}<a href={b.button_link} class="mt-6 inline-block rounded-full bg-white px-6 py-3 font-semibold" style="color:{color}">{b.button_label}</a>{/if}
			</div>
		</section>
	{:else if b.kind === 'contact'}
		<section class="mx-auto max-w-5xl px-6 py-12" data-block="contact">
			{#if b.heading}<h2 class="mb-6 text-3xl font-bold text-slate-900">{b.heading}</h2>{/if}
			<dl class="grid gap-6 sm:grid-cols-2">
				{#if b.address}<div><dt class="text-sm text-slate-500">Address</dt><dd class="whitespace-pre-line text-slate-900">{b.address}</dd></div>{/if}
				{#if b.phone}<div><dt class="text-sm text-slate-500">Phone</dt><dd><a class="text-slate-900 hover:underline" href="tel:{b.phone.replace(/\s/g, '')}">{b.phone}</a></dd></div>{/if}
				{#if b.email}<div><dt class="text-sm text-slate-500">Email</dt><dd><a class="text-slate-900 hover:underline" href="mailto:{b.email}">{b.email}</a></dd></div>{/if}
				{#if b.hours}<div><dt class="text-sm text-slate-500">Opening hours</dt><dd class="whitespace-pre-line text-slate-900">{b.hours}</dd></div>{/if}
			</dl>
		</section>
	{:else if b.kind === 'form'}
		<section class="mx-auto max-w-2xl px-6 py-10" data-block="form">
			{#if b.heading}<h2 class="mb-6 text-center text-3xl font-bold text-slate-900">{b.heading}</h2>{/if}
			{#if b.form_key}<iframe src="/f/{encodeURIComponent(b.form_key)}?embed=1" title={b.heading ?? 'Form'} class="w-full border-0" style="height:{heights[b.form_key] ?? 520}px"></iframe>{/if}
		</section>
	{/if}
{/each}
