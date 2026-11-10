<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { replaceState } from '$app/navigation';
	import { api, downloadFile, openFile, query, requestBlob } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { crm, money } from '#lib/crm.svelte.ts';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Doc = {
		id: string;
		item_id: string;
		number: string;
		kind: string;
		template_name: string;
		engine: string;
		status: 'pending' | 'ready' | 'failed';
		error: string | null;
		pdf_file_id: string | null;
		source_file_id: string | null;
		created_at: string;
		created_by_name: string | null;
		type_code: string;
		type_name: string;
		item_number: number;
		item_title: string;
		client: string | null;
		amount: string | null;
		currency: string | null;
	};
	const PAGE = 50;

	// Filters live in the address, so a filtered list (e.g. ?kind=Invoice) can be bookmarked.
	const p0 = page.url.searchParams;
	let q = $state(p0.get('q') ?? '');
	let kind = $state(p0.get('kind') ?? '');
	let type = $state(p0.get('type') ?? '');
	let status = $state(p0.get('status') ?? '');
	let from = $state(p0.get('from') ?? '');
	let to = $state(p0.get('to') ?? '');
	let offset = $state(0);

	let items = $state<Doc[]>([]);
	let total = $state(0);
	let capped = $state(false);
	let kinds = $state<string[]>([]);
	let loading = $state(true);
	let selected = $state<Set<string>>(new Set());
	let zipping = $state(false);

	const filters = () => ({ q: q.trim() || undefined, kind: kind || undefined, type: type || undefined, status: status || undefined, from: from || undefined, to: to || undefined });
	const ready = $derived(items.filter((d) => d.status === 'ready' && d.pdf_file_id));
	const allChecked = $derived(ready.length > 0 && ready.every((d) => selected.has(d.id)));

	async function load() {
		loading = true;
		try {
			const r = await api.get<{ items: Doc[]; total: number; total_capped: boolean; kinds: string[] }>('/crm/documents' + query({ ...filters(), limit: PAGE, offset }));
			items = r.items;
			total = r.total;
			capped = r.total_capped;
			kinds = r.kinds;
		} catch (e) {
			toasts.error(e);
		} finally {
			loading = false;
		}
	}
	function changed() {
		offset = 0;
		selected = new Set();
		try {
			replaceState('/crm/documents' + query(filters()), {});
		} catch {
			/* router not ready yet */
		}
		load();
	}
	let t: ReturnType<typeof setTimeout>;
	function typed() {
		clearTimeout(t);
		t = setTimeout(changed, 250);
	}
	function clear() {
		q = kind = type = status = from = to = '';
		changed();
	}
	function go(delta: number) {
		offset = Math.max(0, offset + delta);
		selected = new Set();
		load();
	}
	function toggle(id: string) {
		const s = new Set(selected);
		s.has(id) ? s.delete(id) : s.add(id);
		selected = s;
	}
	function toggleAll() {
		selected = allChecked ? new Set() : new Set(ready.map((d) => d.id));
	}
	async function downloadZip() {
		zipping = true;
		try {
			const blob = await requestBlob('POST', '/crm/documents/zip', { ids: [...selected] });
			const href = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = href;
			a.download = `documents-${new Date().toISOString().slice(0, 10)}.zip`;
			document.body.appendChild(a);
			a.click();
			a.remove();
			setTimeout(() => URL.revokeObjectURL(href), 60_000);
		} catch (e) {
			toasts.error(e);
		} finally {
			zipping = false;
		}
	}
	onMount(async () => {
		await crm.load();
		load();
	});
	const filtered = $derived(!!(q || kind || type || status || from || to));
</script>

<svelte:head><title>Documents · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Documents</h1>
		<span class="text-sm text-slate-500">{total}{capped ? '+' : ''}</span>
		<div class="ml-auto flex items-center gap-2">
			{#if selected.size}
				<button class="btn btn-ghost" onclick={() => (selected = new Set())}>Clear selection</button>
				<button class="btn btn-primary" disabled={zipping} onclick={downloadZip}><Icon name="download" class="size-4" /> {zipping ? 'Preparing…' : `Download ${selected.size} PDF${selected.size === 1 ? '' : 's'} (ZIP)`}</button>
			{/if}
		</div>
	</div>

	<div class="flex flex-wrap items-end gap-3">
		<div class="w-full sm:w-72">
			<label class="label" for="d-q">Search</label>
			<input id="d-q" class="input" placeholder="Number, record or company" bind:value={q} oninput={typed} />
		</div>
		<div>
			<label class="label" for="d-k">Kind</label>
			<select id="d-k" class="input" bind:value={kind} onchange={changed}>
				<option value="">All kinds</option>
				{#each kinds as k (k)}<option>{k}</option>{/each}
			</select>
		</div>
		<div>
			<label class="label" for="d-t">Record type</label>
			<select id="d-t" class="input" bind:value={type} onchange={changed}>
				<option value="">All types</option>
				{#each crm.config?.types ?? [] as ct (ct.code)}<option value={ct.code}>{ct.name_plural}</option>{/each}
			</select>
		</div>
		<div>
			<label class="label" for="d-s">Status</label>
			<select id="d-s" class="input" bind:value={status} onchange={changed}>
				<option value="">Any</option>
				<option value="ready">Ready</option>
				<option value="pending">Being created</option>
				<option value="failed">Failed</option>
			</select>
		</div>
		<div>
			<label class="label" for="d-f">From</label>
			<input id="d-f" type="date" class="input" bind:value={from} onchange={changed} />
		</div>
		<div>
			<label class="label" for="d-to">To</label>
			<input id="d-to" type="date" class="input" bind:value={to} onchange={changed} />
		</div>
		{#if filtered}<button class="btn btn-ghost" onclick={clear}><Icon name="x" class="size-4" /> Clear</button>{/if}
	</div>

	<div class="card overflow-x-auto">
		<table class="min-w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500">
				<tr>
					<th class="w-10 px-4 py-3"><input type="checkbox" class="rounded border-slate-300 text-brand-600" aria-label="Select all with a PDF" checked={allChecked} disabled={!ready.length} onchange={toggleAll} /></th>
					<th class="px-4 py-3">Number</th>
					<th class="px-4 py-3">Kind</th>
					<th class="px-4 py-3">Record</th>
					<th class="px-4 py-3">Client</th>
					<th class="px-4 py-3 text-right">Record amount</th>
					<th class="px-4 py-3">Created</th>
					<th class="px-4 py-3"></th>
				</tr>
			</thead>
			<tbody class="divide-y divide-slate-50">
				{#each items as d (d.id)}
					{@const hasPdf = d.status === 'ready' && !!d.pdf_file_id}
					<tr class="hover:bg-slate-50/60">
						<td class="px-4 py-2.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" aria-label="Select {d.number}" disabled={!hasPdf} checked={selected.has(d.id)} onchange={() => toggle(d.id)} /></td>
						<td class="px-4 py-2.5 whitespace-nowrap">
							{#if hasPdf}
								<button class="font-medium text-slate-900 hover:text-brand-700" title="Open the PDF" onclick={() => openFile(d.pdf_file_id!).catch(toasts.error)}>{d.number}</button>
							{:else}
								<span class="font-medium text-slate-500">{d.number}</span>
							{/if}
							{#if d.status === 'pending'}<span class="badge ml-2 bg-amber-50 text-amber-800">being created</span>{/if}
							{#if d.status === 'failed'}<span class="badge ml-2 bg-red-50 text-red-700" title={d.error ?? ''}>failed</span>{/if}
						</td>
						<td class="px-4 py-2.5"><span>{d.kind}</span><div class="text-xs text-slate-400">{d.template_name}</div></td>
						<td class="px-4 py-2.5">
							<a class="text-slate-800 hover:text-brand-700" href="/crm/{d.type_code}/{d.item_id}">{d.item_title}</a>
							<div class="text-xs text-slate-400">{d.type_name} #{d.item_number}</div>
						</td>
						<td class="px-4 py-2.5 text-slate-600">{d.client ?? '—'}</td>
						<td class="px-4 py-2.5 text-right whitespace-nowrap">{d.amount && d.currency ? money(d.amount, d.currency) : '—'}</td>
						<td class="px-4 py-2.5 whitespace-nowrap text-slate-600">{formatDate(d.created_at)}<div class="text-xs text-slate-400">{d.created_by_name ?? ''}</div></td>
						<td class="px-4 py-2.5 text-right whitespace-nowrap">
							{#if hasPdf}
								<button class="btn btn-ghost p-1.5" title="Open the PDF" aria-label="Open {d.number}" onclick={() => openFile(d.pdf_file_id!).catch(toasts.error)}><Icon name="eye" class="size-4" /></button>
								<button class="btn btn-ghost p-1.5" title="Download the PDF" aria-label="Download {d.number}" onclick={() => downloadFile(d.pdf_file_id!, `${d.number}.pdf`).catch(toasts.error)}><Icon name="download" class="size-4" /></button>
							{/if}
							{#if d.source_file_id}
								<button class="btn btn-ghost px-1.5 py-1 text-xs font-medium" title="Download the filled Word file" onclick={() => downloadFile(d.source_file_id!, `${d.number}.docx`).catch(toasts.error)}>DOCX</button>
							{/if}
						</td>
					</tr>
				{:else}
					<tr>
						<td colspan="8" class="px-4 py-14 text-center text-slate-500">
							{#if loading}Loading…{:else if filtered}No documents match these filters.{:else}No documents yet. Create one from the Documents card on a deal, lead or other record.{/if}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>

	{#if total > PAGE}
		<div class="flex items-center justify-end gap-3 text-sm text-slate-600">
			<span>{offset + 1}–{Math.min(offset + PAGE, total)} of {total}{capped ? '+' : ''}</span>
			<button class="btn btn-secondary" disabled={offset === 0} onclick={() => go(-PAGE)}>Previous</button>
			<button class="btn btn-secondary" disabled={offset + PAGE >= total} onclick={() => go(PAGE)}>Next</button>
		</div>
	{/if}
</div>
