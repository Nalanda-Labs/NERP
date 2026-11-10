<script lang="ts">
	// The API reference, read from /api/openapi.json: endpoints by area (parameters, request
	// body, needed scope) and the webhook events with their payloads.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Schema = { type?: string; format?: string; description?: string; properties?: Record<string, Schema>; items?: Schema; required?: string[] };
	type Param = { name: string; in: string; description: string; required: boolean; schema: Schema };
	type Op = {
		summary: string; description: string; tags?: string[]; parameters?: Param[];
		requestBody?: { content: Record<string, { schema: Schema; example?: unknown }> };
	};
	type Spec = { info: { title: string; description: string }; paths: Record<string, Record<string, Op>>; webhooks: Record<string, Record<string, Op>>; tags: { name: string }[] };

	let spec = $state<Spec | null>(null);
	let open = $state<string | null>(null);
	let filter = $state('');
	onMount(async () => {
		try {
			spec = await api.get<Spec>('/openapi.json');
		} catch (e) {
			toasts.error(e);
		}
	});

	const origin = typeof location !== 'undefined' ? location.origin : '';
	/** Minimal formatting for the spec's texts: paragraphs, `code` and **bold** (escaped first). */
	function md(text: string): string {
		const esc = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
		return esc
			.split(/\n\n+/)
			.map((p) => `<p>${p.replace(/`([^`]+)`/g, '<code class="rounded bg-slate-100 px-1 text-[0.85em]">$1</code>').replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>').replace(/\n/g, '<br>')}</p>`)
			.join('');
	}
	const METHOD: Record<string, string> = { get: 'bg-sky-100 text-sky-800', post: 'bg-emerald-100 text-emerald-800', put: 'bg-amber-100 text-amber-800', patch: 'bg-amber-100 text-amber-800', delete: 'bg-red-100 text-red-800' };
	const ops = $derived(
		spec
			? Object.entries(spec.paths).flatMap(([path, m]) => Object.entries(m).map(([method, op]) => ({ key: `${method} ${path}`, method, path, op })))
			: []
	);
	const shown = $derived(ops.filter((o) => !filter.trim() || `${o.method} ${o.path} ${o.op.summary}`.toLowerCase().includes(filter.trim().toLowerCase())));
	const typeOf = (s: Schema): string => (s.type === 'array' ? `${typeOf(s.items ?? {})}[]` : (s.format ?? s.type ?? 'any'));
</script>

{#snippet props(schema: Schema)}
	{#if schema.properties}
		<table class="w-full text-xs">
			<tbody class="divide-y divide-slate-100">
				{#each Object.entries(schema.properties) as [name, p] (name)}
					<tr>
						<td class="py-1 pr-3 align-top font-mono whitespace-nowrap text-slate-800">{name}{#if schema.required?.includes(name)}<span class="text-red-600">*</span>{/if}</td>
						<td class="py-1 pr-3 align-top font-mono whitespace-nowrap text-slate-500">{typeOf(p)}</td>
						<td class="py-1 align-top text-slate-600">{p.description ?? ''}{#if p.properties}<div class="mt-1 border-l border-slate-200 pl-2">{@render props(p)}</div>{/if}{#if p.items?.properties}<div class="mt-1 border-l border-slate-200 pl-2">{@render props(p.items)}</div>{/if}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
{/snippet}

{#snippet operation(key: string, method: string, path: string, op: Op)}
	{@const body = op.requestBody?.content['application/json'] ?? op.requestBody?.content['multipart/form-data']}
	<li data-op={key}>
		<button class="flex w-full items-center gap-3 px-4 py-2.5 text-left hover:bg-slate-50" onclick={() => (open = open === key ? null : key)} aria-expanded={open === key}>
			<span class="w-16 rounded px-1.5 py-0.5 text-center font-mono text-xs font-semibold uppercase {METHOD[method] ?? 'bg-slate-100 text-slate-700'}">{method}</span>
			<code class="text-sm text-slate-900">{path}</code>
			<span class="truncate text-sm text-slate-500">{op.summary}</span>
		</button>
		{#if open === key}
			<div class="space-y-3 border-t border-slate-100 bg-slate-50/60 px-4 py-3 text-sm text-slate-700">
				{#if op.description}<div class="space-y-2">{@html md(op.description)}</div>{/if}
				{#if op.parameters?.length}
					<div>
						<p class="mb-1 text-xs font-medium text-slate-500 uppercase">Parameters</p>
						<table class="w-full text-xs">
							<tbody class="divide-y divide-slate-100">
								{#each op.parameters as p (p.in + p.name)}
									<tr><td class="py-1 pr-3 font-mono whitespace-nowrap">{p.name}{#if p.required}<span class="text-red-600">*</span>{/if}</td><td class="py-1 pr-3 whitespace-nowrap text-slate-500">{p.in} · {typeOf(p.schema)}</td><td class="py-1 text-slate-600">{p.description}</td></tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
				{#if body}
					<div>
						<p class="mb-1 text-xs font-medium text-slate-500 uppercase">Body</p>
						{@render props(body.schema)}
						{#if body.example}<pre class="mt-2 overflow-x-auto rounded-lg bg-slate-900 p-2 text-xs text-slate-100">{JSON.stringify(body.example, null, 2)}</pre>{/if}
					</div>
				{/if}
			</div>
		{/if}
	</li>
{/snippet}

{#if spec}
	<div class="space-y-5">
		<section class="card space-y-2 p-5 text-sm text-slate-700">
			{@html md(spec.info.description)}
			<p>Base URL: <code class="rounded bg-slate-100 px-1">{origin}/api</code> · Machine-readable: <a class="text-brand-700 hover:underline" href="/api/openapi.json" target="_blank" rel="noopener">openapi.json</a> (OpenAPI 3.1, for code generators and API tools).</p>
		</section>
		<input class="input max-w-sm" type="search" placeholder="Filter endpoints…" bind:value={filter} aria-label="Filter endpoints" />
		{#each spec.tags as tag (tag.name)}
			{@const list = shown.filter((o) => o.op.tags?.includes(tag.name))}
			{#if list.length}
				<section class="card overflow-hidden" aria-label={tag.name}>
					<h2 class="border-b border-slate-100 px-4 py-2.5 font-medium text-slate-900">{tag.name}</h2>
					<ul class="divide-y divide-slate-100">
						{#each list as o (o.key)}{@render operation(o.key, o.method, o.path, o.op)}{/each}
					</ul>
				</section>
			{/if}
		{/each}
		<section class="card overflow-hidden" aria-label="Webhook events">
			<h2 class="border-b border-slate-100 px-4 py-2.5 font-medium text-slate-900">Webhook events</h2>
			<ul class="divide-y divide-slate-100">
				{#each Object.entries(spec.webhooks) as [name, m] (name)}{@render operation(`hook ${name}`, 'event', name, m.post)}{/each}
			</ul>
		</section>
	</div>
{:else}
	<p class="text-sm text-slate-400">Loading…</p>
{/if}
