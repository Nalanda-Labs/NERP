<script lang="ts">
	// A website page: its blocks on the left, the preview on the right; save, publish.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, requestBlob } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import SiteBlocks, { type Block } from '#lib/components/SiteBlocks.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type P = { id: string; site_id: string; path: string; title: string; description: string | null; blocks: Block[]; rendered: Block[]; in_menu: boolean; published_at: string | null; changed: boolean; url: string };
	let p = $state<P | null>(null);
	let blocks = $state<Block[]>([]);
	let meta = $state({ title: '', path: '', description: '', in_menu: true });
	let preview = $state<Block[]>([]);
	let color = $state('#1a73e8');
	let dirty = $state(false);
	let busy = $state(false);
	let forms = $state<{ key: string; name: string }[]>([]);
	// Preview images: the editor's files as blob addresses.
	let urls = $state<Record<string, string>>({});
	function want(id: string | undefined) {
		if (!id || urls[id] !== undefined) return;
		urls[id] = '';
		requestBlob('GET', `/files/${id}/content`).then((b) => (urls[id] = URL.createObjectURL(b))).catch(() => {});
	}
	const src = (id: string) => urls[id] || null;

	function take(x: P) {
		p = x;
		blocks = structuredClone(x.blocks);
		meta = { title: x.title, path: x.path, description: x.description ?? '', in_menu: x.in_menu };
		preview = x.rendered;
		dirty = false;
		for (const b of x.blocks) {
			want(b.image);
			b.images?.forEach(want);
		}
	}
	$effect(() => {
		api.get<P>(`/sites/pages/${page.params.id}`).then(async (x) => {
			take(x);
			color = (await api.get<{ settings: { color: string } }>(`/sites/${x.site_id}`)).settings.color ?? color;
		}).catch((e) => toasts.error(e));
	});
	onMount(() => {
		api.get<{ items: { key: string; name: string }[] }>('/crm/webforms').then((r) => (forms = r.items)).catch(() => {});
	});

	// The preview follows the blocks (text blocks are rendered by the server).
	let t: ReturnType<typeof setTimeout> | undefined;
	function changed() {
		dirty = true;
		clearTimeout(t);
		t = setTimeout(async () => {
			const out: Block[] = [];
			for (const b of blocks) {
				out.push(b.kind === 'text' ? { ...b, html: (await api.post<{ html: string }>('/kb/preview', { body: b.markdown ?? '' })).html } : { ...b });
			}
			preview = out;
		}, 300);
	}
	const NEW: Record<Block['kind'], Block> = {
		hero: { kind: 'hero', heading: 'A headline', text: 'One sentence about it.', button_label: 'Contact us', button_link: '#contact' },
		text: { kind: 'text', markdown: 'Some text.' },
		features: { kind: 'features', heading: 'Why us', items: [{ title: 'Fast', text: 'We reply within a day.' }, { title: 'Local', text: 'Based in your city.' }, { title: 'Fair', text: 'Clear prices.' }] },
		image: { kind: 'image' },
		gallery: { kind: 'gallery', images: [] },
		cta: { kind: 'cta', heading: 'Ready to start?', button_label: 'Get in touch', button_link: '#contact' },
		contact: { kind: 'contact', heading: 'Contact', address: '', phone: '', email: '' },
		form: { kind: 'form', heading: 'Write to us', form_key: '' }
	};
	const NAMES: Record<Block['kind'], string> = { hero: 'Banner', text: 'Text', features: 'Features', image: 'Image', gallery: 'Gallery', cta: 'Call to action', contact: 'Contact details', form: 'Web form' };
	let adding = $state<Block['kind']>('text');
	function add() {
		blocks.push(structuredClone(NEW[adding]));
		changed();
	}
	function move(i: number, d: number) {
		const j = i + d;
		if (j < 0 || j >= blocks.length) return;
		[blocks[i], blocks[j]] = [blocks[j], blocks[i]];
		changed();
	}
	async function upload(e: Event, set: (id: string) => void) {
		const input = e.currentTarget as HTMLInputElement;
		for (const f of Array.from(input.files ?? [])) {
			const fd = new FormData();
			fd.append('file', f);
			try {
				const up = await api.post<{ id: string }>('/files', fd);
				urls[up.id] = URL.createObjectURL(f);
				set(up.id);
			} catch (err) {
				toasts.error(err);
			}
		}
		input.value = '';
		changed();
	}
	async function save(): Promise<boolean> {
		busy = true;
		try {
			take(await api.patch<P>(`/sites/pages/${p!.id}`, { ...meta, description: meta.description || null, path: p!.path === '' ? undefined : meta.path, blocks }));
			return true;
		} catch (e) {
			toasts.error(e);
			return false;
		} finally {
			busy = false;
		}
	}
	async function publish() {
		if (dirty && !(await save())) return;
		take(await api.post<P>(`/sites/pages/${p!.id}/publish`, {}));
		toasts.success('Published');
	}
	async function remove() {
		if (!confirm(`Delete the page “${p!.title}”?`)) return;
		try {
			await api.del(`/sites/pages/${p!.id}`);
			goto(`/sites/${p!.site_id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>{p?.title ?? 'Page'} · NERP</title></svelte:head>

{#if p}
	<div class="mb-4 flex flex-wrap items-center gap-3">
		<a href="/sites/{p.site_id}" class="text-sm text-slate-500 hover:text-slate-800">← Website</a>
		<h1 class="flex-1 text-xl font-semibold text-slate-900">{p.title}</h1>
		{#if p.published_at}<a class="text-sm text-brand-600 hover:underline" href={p.url} target="_blank" rel="noopener">View published</a>{/if}
		<span class="text-sm text-slate-500" data-state>{dirty ? 'Unsaved changes' : !p.published_at ? 'Draft' : p.changed ? 'Changes not published' : 'Published'}</span>
		<button class="btn btn-secondary" disabled={busy || !dirty} onclick={save}>Save</button>
		<button class="btn btn-primary" disabled={busy} onclick={publish}>Publish</button>
	</div>
	<div class="grid gap-5 xl:grid-cols-[26rem_minmax(0,1fr)]">
		<div class="space-y-3">
			<div class="card space-y-3 p-4">
				<div class="grid grid-cols-2 gap-3">
					<div><label class="label" for="pg-t">Title</label><input id="pg-t" class="input" bind:value={meta.title} oninput={() => (dirty = true)} /></div>
					<div><label class="label" for="pg-p">Address</label><input id="pg-p" class="input" disabled={p.path === ''} placeholder="home" bind:value={meta.path} oninput={() => (dirty = true)} /></div>
				</div>
				<div><label class="label" for="pg-d">Description for search engines</label><input id="pg-d" class="input" maxlength="300" bind:value={meta.description} oninput={() => (dirty = true)} /></div>
				<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={meta.in_menu} onchange={() => (dirty = true)} /> In the menu</label>
			</div>
			{#each blocks as b, i (i)}
				<div class="card space-y-2 p-4" data-edit-block={b.kind}>
					<div class="flex items-center gap-2"><span class="flex-1 text-sm font-medium text-slate-800">{NAMES[b.kind]}</span>
						<button class="btn btn-ghost p-1" aria-label="Move up" onclick={() => move(i, -1)}>↑</button><button class="btn btn-ghost p-1" aria-label="Move down" onclick={() => move(i, 1)}>↓</button>
						<button class="btn btn-ghost p-1 hover:text-red-600" aria-label="Remove block" onclick={() => (blocks.splice(i, 1), changed())}><Icon name="trash" class="size-4" /></button></div>
					{#if b.kind === 'text'}
						<textarea class="input min-h-32 font-mono text-sm" aria-label="Text (Markdown)" bind:value={b.markdown} oninput={changed}></textarea>
					{:else if b.kind === 'hero' || b.kind === 'cta'}
						<input class="input" placeholder="Heading" aria-label="Heading" bind:value={b.heading} oninput={changed} />
						<textarea class="input" rows="2" placeholder="Text" aria-label="Text" bind:value={b.text} oninput={changed}></textarea>
						<div class="grid grid-cols-2 gap-2"><input class="input" placeholder="Button text" aria-label="Button text" bind:value={b.button_label} oninput={changed} /><input class="input" placeholder="Button link: /page, https://…" aria-label="Button link" bind:value={b.button_link} oninput={changed} /></div>
						{#if b.kind === 'hero'}<label class="btn btn-secondary cursor-pointer px-3 py-1 text-sm">{b.image ? 'Replace background' : 'Background picture'}<input type="file" accept="image/*" class="sr-only" onchange={(e) => upload(e, (id) => (b.image = id))} /></label>{/if}
					{:else if b.kind === 'features'}
						<input class="input" placeholder="Heading" aria-label="Heading" bind:value={b.heading} oninput={changed} />
						{#each b.items ?? [] as f, j (j)}<div class="flex gap-2"><input class="input w-32" placeholder="Title" bind:value={f.title} oninput={changed} /><input class="input flex-1" placeholder="Text" bind:value={f.text} oninput={changed} /><button class="text-slate-400 hover:text-red-600" aria-label="Remove" onclick={() => (b.items!.splice(j, 1), changed())}>×</button></div>{/each}
						<button class="text-sm text-brand-600 hover:underline" onclick={() => (b.items!.push({ title: '', text: '' }), changed())}>+ Feature</button>
					{:else if b.kind === 'image'}
						<label class="btn btn-secondary cursor-pointer px-3 py-1 text-sm">{b.image ? 'Replace picture' : 'Choose a picture'}<input type="file" accept="image/*" class="sr-only" aria-label="Picture" onchange={(e) => upload(e, (id) => (b.image = id))} /></label>
						<input class="input" placeholder="Caption" aria-label="Caption" bind:value={b.caption} oninput={changed} />
					{:else if b.kind === 'gallery'}
						<p class="text-xs text-slate-500">{b.images?.length ?? 0} pictures</p>
						<label class="btn btn-secondary cursor-pointer px-3 py-1 text-sm">Add pictures<input type="file" accept="image/*" multiple class="sr-only" onchange={(e) => upload(e, (id) => b.images!.push(id))} /></label>
					{:else if b.kind === 'contact'}
						<input class="input" placeholder="Heading" bind:value={b.heading} oninput={changed} />
						<textarea class="input" rows="2" placeholder="Address" bind:value={b.address} oninput={changed}></textarea>
						<div class="grid grid-cols-2 gap-2"><input class="input" placeholder="Phone" bind:value={b.phone} oninput={changed} /><input class="input" placeholder="Email" bind:value={b.email} oninput={changed} /></div>
						<input class="input" placeholder="Opening hours" bind:value={b.hours} oninput={changed} />
					{:else if b.kind === 'form'}
						<input class="input" placeholder="Heading" bind:value={b.heading} oninput={changed} />
						{#if forms.length}<select class="input" aria-label="Web form" bind:value={b.form_key} onchange={changed}><option value="">Choose a web form…</option>{#each forms as f (f.key)}<option value={f.key}>{f.name}</option>{/each}</select>
						{:else}<input class="input" placeholder="Web form key (from CRM → Web forms)" aria-label="Web form key" bind:value={b.form_key} oninput={changed} />{/if}
						<p class="text-xs text-slate-500">What visitors send goes to the CRM as the form says.</p>
					{/if}
				</div>
			{/each}
			<div class="flex gap-2"><select class="input flex-1" aria-label="Block to add" bind:value={adding}>{#each Object.entries(NAMES) as [k, n] (k)}<option value={k}>{n}</option>{/each}</select><button class="btn btn-secondary" onclick={add}>Add block</button></div>
			{#if p.path !== ''}<button class="text-sm text-slate-400 hover:text-red-600" onclick={remove}>Delete this page</button>{/if}
		</div>
		<div class="card overflow-hidden" aria-label="Preview">
			<div class="border-b border-slate-100 px-4 py-2 text-xs text-slate-500">Preview</div>
			<div class="max-h-[80vh] overflow-y-auto" data-preview><SiteBlocks blocks={preview} {color} {src} /></div>
		</div>
	</div>
{/if}
