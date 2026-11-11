<script lang="ts" module>
	export type Block = {
		kind: 'heading' | 'text' | 'button' | 'image' | 'divider' | 'spacer' | 'columns' | 'social';
		text?: string; size?: 'large' | 'small'; align?: 'left' | 'center' | 'right';
		label?: string; url?: string; color?: string;
		file_id?: string | null; alt?: string; link?: string; width?: number;
		height?: number; left?: string; right?: string;
		links?: { network: string; url: string }[];
	};
	export type Design = { style: { background: string; content: string; text: string; accent: string; font: 'sans' | 'serif' | 'mono'; preheader: string }; blocks: Block[] };
	export const blankDesign = (): Design => ({
		style: { background: '#f1f5f9', content: '#ffffff', text: '#1e293b', accent: '#1a73e8', font: 'sans', preheader: '' },
		blocks: [
			{ kind: 'heading', text: 'Hello {{first_name}}', size: 'large', align: 'left' },
			{ kind: 'text', text: 'Write your message here. **Bold**, *italic* and links like [our offer](https://yourcompany.com) work.', align: 'left' },
			{ kind: 'button', label: 'See the offer', url: 'https://yourcompany.com', align: 'center' }
		]
	});
</script>

<script lang="ts">
	// The email designer: blocks dragged (or clicked) in from the palette, reordered by dragging,
	// each set up on the right; the email as it will look in the middle (rendered by the server,
	// exactly as it is sent).
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';

	let { design = $bindable(), onchange }: { design: Design; onchange?: () => void } = $props();
	const PALETTE: [Block['kind'], string][] = [['heading', 'Heading'], ['text', 'Text'], ['button', 'Button'], ['image', 'Image'], ['columns', 'Two columns'], ['divider', 'Divider'], ['spacer', 'Space'], ['social', 'Social links']];
	const NETWORKS = ['facebook', 'instagram', 'linkedin', 'x', 'youtube', 'tiktok', 'website'];
	let selected = $state<number | null>(0);
	let html = $state('');
	let error = $state<string | null>(null);
	let drag = $state<{ from?: number; kind?: Block['kind'] } | null>(null);
	let over = $state<number | null>(null);

	function fresh(kind: Block['kind']): Block {
		switch (kind) {
			case 'heading': return { kind, text: 'A heading', size: 'large', align: 'left' };
			case 'text': return { kind, text: 'Some text.', align: 'left' };
			case 'button': return { kind, label: 'Click here', url: 'https://', align: 'center' };
			case 'image': return { kind, url: '', file_id: null, alt: '', link: '', width: 100, align: 'center' };
			case 'spacer': return { kind, height: 24 };
			case 'columns': return { kind, left: 'Left column', right: 'Right column' };
			case 'social': return { kind, links: [{ network: 'website', url: 'https://' }], align: 'center' };
			default: return { kind };
		}
	}
	function changed() {
		onchange?.();
		schedule();
	}
	function add(kind: Block['kind'], at = design.blocks.length) {
		design.blocks.splice(at, 0, fresh(kind));
		selected = at;
		changed();
	}
	function move(from: number, to: number) {
		if (to < 0 || to > design.blocks.length || from === to) return;
		const [b] = design.blocks.splice(from, 1);
		design.blocks.splice(to > from ? to - 1 : to, 0, b!);
		selected = to > from ? to - 1 : to;
		changed();
	}
	function drop(at: number) {
		if (drag?.kind) add(drag.kind, at);
		else if (drag?.from !== undefined) move(drag.from, at);
		drag = null;
		over = null;
	}
	function remove(i: number) {
		design.blocks.splice(i, 1);
		selected = design.blocks.length ? Math.min(i, design.blocks.length - 1) : null;
		changed();
	}
	function duplicate(i: number) {
		design.blocks.splice(i + 1, 0, structuredClone($state.snapshot(design.blocks[i]!)) as Block);
		selected = i + 1;
		changed();
	}

	let timer: ReturnType<typeof setTimeout>;
	function schedule() {
		clearTimeout(timer);
		timer = setTimeout(preview, 400);
	}
	async function preview() {
		try {
			html = (await api.post<{ html: string }>('/marketing/design/preview', { design: $state.snapshot(design) })).html;
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}
	$effect(() => {
		preview();
	});

	async function upload(b: Block, f: File | undefined) {
		if (!f) return;
		const fd = new FormData();
		fd.append('file', f);
		try {
			const meta = await api.post<{ id: string }>('/files', fd);
			b.file_id = meta.id;
			b.url = '';
			if (!b.alt) b.alt = f.name.replace(/\.[^.]+$/, '');
			changed();
		} catch (e) {
			toasts.error(e);
		}
	}
	const summary = (b: Block) =>
		b.kind === 'heading' || b.kind === 'text' ? (b.text ?? '').slice(0, 60) : b.kind === 'button' ? b.label : b.kind === 'image' ? b.alt || (b.file_id ? 'Uploaded image' : b.url) : b.kind === 'columns' ? (b.left ?? '').slice(0, 30) : b.kind === 'social' ? (b.links ?? []).map((l) => l.network).join(', ') : '';
	const current = $derived(selected === null ? null : design.blocks[selected] ?? null);
</script>

<div class="grid gap-3 lg:grid-cols-[13rem_minmax(0,1fr)_17rem]" data-email-designer>
	<!-- Palette and outline -->
	<div class="space-y-3">
		<div>
			<p class="label">Add a block</p>
			<div class="grid grid-cols-2 gap-1">
				{#each PALETTE as [k, l] (k)}
					<button type="button" draggable="true" class="rounded-lg border border-slate-200 px-2 py-1.5 text-left text-xs hover:border-brand-400 hover:bg-brand-50" ondragstart={() => (drag = { kind: k })} ondragend={() => ((drag = null), (over = null))} onclick={() => add(k)} data-palette={k}>{l}</button>
				{/each}
			</div>
		</div>
		<div>
			<p class="label">Blocks</p>
			<ol class="space-y-0.5" aria-label="Blocks">
				{#each design.blocks as b, i (i)}
					<li
						class="group flex items-center gap-1 rounded-lg border px-2 py-1 text-xs {selected === i ? 'border-brand-400 bg-brand-50' : 'border-transparent hover:bg-slate-50'} {over === i ? 'border-t-2 border-t-brand-500' : ''}"
						draggable="true"
						ondragstart={() => (drag = { from: i })}
						ondragover={(e) => (e.preventDefault(), (over = i))}
						ondrop={(e) => (e.preventDefault(), drop(i))}
						data-block={b.kind}
					>
						<button type="button" class="min-w-0 flex-1 truncate text-left" onclick={() => (selected = i)}><span class="font-medium text-slate-700">{PALETTE.find((p) => p[0] === b.kind)?.[1]}</span> <span class="text-slate-400">{summary(b)}</span></button>
						<button type="button" class="text-slate-300 hover:text-slate-700" aria-label="Move up" onclick={() => move(i, i - 1)} disabled={i === 0}>↑</button>
						<button type="button" class="text-slate-300 hover:text-slate-700" aria-label="Move down" onclick={() => move(i, i + 2)} disabled={i === design.blocks.length - 1}>↓</button>
					</li>
				{/each}
				<li class="h-6 rounded-lg {over === design.blocks.length ? 'border-t-2 border-t-brand-500' : ''}" ondragover={(e) => (e.preventDefault(), (over = design.blocks.length))} ondrop={(e) => (e.preventDefault(), drop(design.blocks.length))} aria-hidden="true"></li>
			</ol>
		</div>
		<button type="button" class="text-xs text-brand-700 hover:underline" onclick={() => (selected = null)}>Page style…</button>
	</div>

	<!-- The email -->
	<div class="min-h-[28rem] overflow-hidden rounded-xl border border-slate-200 bg-white">
		{#if error}<p class="bg-amber-50 px-3 py-2 text-xs text-amber-800" data-design-error>{error}</p>{/if}
		<iframe title="Email preview" class="h-[36rem] w-full" sandbox="" srcdoc={html} data-design-preview></iframe>
	</div>

	<!-- Properties -->
	<div class="space-y-3 text-sm">
		{#if current}
			{@const b = current}
			<div class="flex items-center gap-2">
				<p class="font-medium text-slate-900">{PALETTE.find((p) => p[0] === b.kind)?.[1]}</p>
				<button type="button" class="btn btn-ghost ml-auto px-2 py-1 text-xs" onclick={() => duplicate(selected!)}>Duplicate</button>
				<button type="button" class="btn btn-ghost p-1 text-slate-400 hover:text-red-600" aria-label="Delete block" onclick={() => remove(selected!)}><Icon name="trash" class="size-4" /></button>
			</div>
			{#if b.kind === 'heading'}
				<div><label class="label" for="d-h">Text</label><input id="d-h" class="input" bind:value={b.text} oninput={changed} /></div>
				<div><label class="label" for="d-hs">Size</label><select id="d-hs" class="input" bind:value={b.size} onchange={changed}><option value="large">Large</option><option value="small">Small</option></select></div>
			{:else if b.kind === 'text'}
				<div><label class="label" for="d-t">Text</label><textarea id="d-t" class="input" rows="8" bind:value={b.text} oninput={changed}></textarea><p class="mt-1 text-xs text-slate-500">**bold**, *italic*, [label](https://…); an empty line starts a paragraph. Placeholders like {'{{first_name}}'} work.</p></div>
			{:else if b.kind === 'button'}
				<div><label class="label" for="d-bl">Label</label><input id="d-bl" class="input" bind:value={b.label} oninput={changed} /></div>
				<div><label class="label" for="d-bu">Link</label><input id="d-bu" class="input" type="url" bind:value={b.url} oninput={changed} /></div>
				<div><label class="label" for="d-bc">Colour</label><input id="d-bc" type="color" class="h-9 w-16 rounded border border-slate-200" bind:value={b.color} oninput={changed} /></div>
			{:else if b.kind === 'image'}
				<div><span class="label">Image</span><input type="file" accept="image/png,image/jpeg,image/gif,image/webp" class="text-xs" aria-label="Upload an image" onchange={(e) => upload(b, e.currentTarget.files?.[0])} /></div>
				<div><label class="label" for="d-iu">…or its web address</label><input id="d-iu" class="input" type="url" placeholder="https://" bind:value={b.url} oninput={() => ((b.file_id = null), changed())} /></div>
				<div><label class="label" for="d-ia">Description</label><input id="d-ia" class="input" bind:value={b.alt} oninput={changed} /></div>
				<div><label class="label" for="d-il">Opens</label><input id="d-il" class="input" type="url" placeholder="https:// (optional)" bind:value={b.link} oninput={changed} /></div>
				<div><label class="label" for="d-iw">Width {b.width}%</label><input id="d-iw" type="range" min="10" max="100" step="5" class="w-full" bind:value={b.width} oninput={changed} /></div>
			{:else if b.kind === 'spacer'}
				<div><label class="label" for="d-sh">Height {b.height}px</label><input id="d-sh" type="range" min="4" max="120" step="4" class="w-full" bind:value={b.height} oninput={changed} /></div>
			{:else if b.kind === 'columns'}
				<div><label class="label" for="d-cl">Left</label><textarea id="d-cl" class="input" rows="5" bind:value={b.left} oninput={changed}></textarea></div>
				<div><label class="label" for="d-cr">Right</label><textarea id="d-cr" class="input" rows="5" bind:value={b.right} oninput={changed}></textarea></div>
			{:else if b.kind === 'social'}
				{#each b.links ?? [] as l, j (j)}
					<div class="flex gap-1">
						<select class="input w-28" bind:value={l.network} onchange={changed} aria-label="Network">{#each NETWORKS as n (n)}<option>{n}</option>{/each}</select>
						<input class="input" type="url" bind:value={l.url} oninput={changed} aria-label="Address" />
						<button type="button" class="text-slate-300 hover:text-red-600" aria-label="Remove" onclick={() => (b.links!.splice(j, 1), changed())}><Icon name="x" class="size-4" /></button>
					</div>
				{/each}
				<button type="button" class="btn btn-ghost px-2 py-1 text-xs" onclick={() => (b.links!.push({ network: 'facebook', url: 'https://' }), changed())}>Add a link</button>
			{/if}
			{#if ['heading', 'text', 'button', 'image', 'social'].includes(b.kind)}
				<div class="flex gap-1" role="radiogroup" aria-label="Alignment">
					{#each ['left', 'center', 'right'] as const as a (a)}
						<button type="button" role="radio" aria-checked={(b.align ?? 'left') === a} class="flex-1 rounded-lg border px-2 py-1 text-xs {(b.align ?? 'left') === a ? 'border-brand-400 bg-brand-50' : 'border-slate-200'}" onclick={() => ((b.align = a), changed())}>{a}</button>
					{/each}
				</div>
			{/if}
		{:else}
			<p class="font-medium text-slate-900">Page style</p>
			<div><label class="label" for="d-pre">Preview text <span class="font-normal text-slate-400">(shown after the subject in inboxes)</span></label><input id="d-pre" class="input" bind:value={design.style.preheader} oninput={changed} /></div>
			<div><label class="label" for="d-font">Font</label><select id="d-font" class="input" bind:value={design.style.font} onchange={changed}><option value="sans">Sans-serif</option><option value="serif">Serif</option><option value="mono">Monospace</option></select></div>
			<div class="grid grid-cols-2 gap-2">
				{#each [['background', 'Background'], ['content', 'Content'], ['text', 'Text'], ['accent', 'Links & buttons']] as const as [k, l] (k)}
					<div><label class="label" for="d-{k}">{l}</label><input id="d-{k}" type="color" class="h-9 w-full rounded border border-slate-200" bind:value={design.style[k]} oninput={changed} /></div>
				{/each}
			</div>
		{/if}
	</div>
</div>
