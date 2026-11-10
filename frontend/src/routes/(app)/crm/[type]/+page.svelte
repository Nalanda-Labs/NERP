<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, downloadUrl, query } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { crm, durationText, invoiceOverdue, money, moneyShort, optionLabel, overLimit, ufText, type CrmRow, type CrmStage, type CrmType, linksCompany } from '#lib/crm.svelte.ts';
	import { formatRelative } from '#lib/format.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Column = { stage: CrmStage; count: number; count_is_capped: boolean; sum_base: string; items: CrmRow[] };
	// Stage settings (time limits) come from the config; the clock re-checks them each minute.
	const stageOf = (id: string) => type?.categories.flatMap((c) => c.stages).find((s) => s.id === id);
	let now = $state(Date.now());
	$effect(() => {
		const t = setInterval(() => (now = Date.now()), 60_000);
		return () => clearInterval(t);
	});
	type Saved = { id: string; name: string; filter: Record<string, string> };

	const code = $derived(page.params.type!);
	let type = $state<CrmType | null>(null);
	let view = $state<'kanban' | 'list'>('list');
	let rows = $state<CrmRow[]>([]);
	let total = $state(0);
	let capped = $state(false);
	let columns = $state<Column[]>([]);
	let offset = $state(0);
	let loading = $state(false);
	let selected = $state<string[]>([]);
	let saved = $state<Saved[]>([]);
	const PAGE = 50;

	// Filter state (mirrors the API's query parameters).
	let f = $state<Record<string, string>>({});
	let q = $state('');
	let showFilters = $state(false);
	let assignedLabel = $state<string | null>(null);
	let assignedId = $state<string | null>(null);

	const base = $derived(crm.config?.base_currency ?? 'USD');
	const category = $derived(type?.categories.find((c) => c.id === f.category_id) ?? type?.categories.find((c) => c.is_default));

	function params(extra: Record<string, unknown> = {}) {
		const qq = q.trim();
		return query({ ...f, q: qq.length >= 3 || /^#?\d+$/.test(qq) ? qq : undefined, ...extra } as Record<string, string>);
	}

	// Only the latest request may update the page: an older, slower one (e.g. the previous
	// pipeline's board) must not overwrite it.
	let loadSeq = 0;
	async function load() {
		if (!type) return;
		const seq = ++loadSeq;
		loading = true;
		try {
			if (view === 'kanban' && type.has_stages) {
				const k = await api.get<{ columns: Column[] }>(`/crm/${code}/kanban` + params({ category_id: category?.id, per_column: 20 }));
				if (seq !== loadSeq) return;
				columns = k.columns;
			} else {
				const p = await api.get<{ items: CrmRow[]; total: number; total_is_capped: boolean }>(`/crm/${code}/items` + params({ limit: PAGE, offset }));
				if (seq !== loadSeq) return;
				rows = p.items;
				total = p.total;
				capped = p.total_is_capped;
			}
		} catch (e) {
			toasts.error(e);
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	async function init() {
		// Reset first: a filter picked while the page is still loading must survive it.
		f = {};
		q = '';
		offset = 0;
		selected = [];
		await crm.load();
		type = crm.type(code) ?? null;
		if (!type) return;
		view = type.has_stages ? (localStorage.getItem(`crm.view.${code}`) as 'kanban' | 'list') ?? 'kanban' : 'list';
		saved = await api.get<Saved[]>(`/crm/${code}/filters`).catch(() => []);
		load();
	}

	$effect(() => {
		code;
		init();
	});

	onMount(() => {
		let t: ReturnType<typeof setTimeout>;
		return realtime.on('crm.changed', (d: { entity_type: number }) => {
			if (d.entity_type !== type?.id) return;
			clearTimeout(t);
			t = setTimeout(load, 500);
		});
	});

	let searchTimer: ReturnType<typeof setTimeout>;
	function onSearch() {
		clearTimeout(searchTimer);
		searchTimer = setTimeout(() => ((offset = 0), load()), 300);
	}
	function setF(k: string, v: string | undefined | null) {
		if (v) f[k] = v;
		else delete f[k];
		offset = 0;
		load();
	}
	function setView(v: 'kanban' | 'list') {
		view = v;
		try {
			localStorage.setItem(`crm.view.${code}`, v);
		} catch {
			/* ignore */
		}
		load();
	}

	async function saveFilter() {
		const name = prompt('Name this filter');
		if (!name) return;
		saved = await api.post<Saved[]>(`/crm/${code}/filters`, { name, filter: { ...f, ...(q.trim() ? { q: q.trim() } : {}) } });
	}
	function applySaved(s: Saved) {
		const { q: sq, ...rest } = s.filter;
		f = { ...rest };
		q = sq ?? '';
		offset = 0;
		load();
	}

	// ----- kanban drag & drop -----
	let dragging = $state<CrmRow | null>(null);
	let overStage = $state<string | null>(null);
	async function drop(stage: CrmStage) {
		const r = dragging;
		dragging = null;
		overStage = null;
		if (!r || r.stage_id === stage.id) return;
		const from = columns.find((c) => c.stage.id === r.stage_id);
		const to = columns.find((c) => c.stage.id === stage.id)!;
		if (from) {
			from.items = from.items.filter((x) => x.id !== r.id);
			from.count--;
		}
		to.items = [{ ...r, stage_id: stage.id }, ...to.items];
		to.count++;
		try {
			await api.post(`/crm/${code}/items/${r.id}/move`, { stage_id: stage.id, before_id: to.items[1]?.id ?? null });
			if (stage.semantics === 'success') toasts.success(`${r.title}: ${stage.name}`);
			load();
		} catch (e) {
			toasts.error(e);
			load();
		}
	}
	async function more(col: Column) {
		const p = await api.get<{ items: CrmRow[] }>(`/crm/${code}/items` + params({ stage_id: col.stage.id, sort: 'position', dir: 'asc', limit: 20, offset: col.items.length }));
		col.items = [...col.items, ...p.items];
	}

	// ----- bulk -----
	let bulkUser = $state<string | null>(null);
	let bulkUserLabel = $state<string | null>(null);
	async function bulk(action: string, value?: unknown) {
		if (action === 'delete' && !confirm(`Move ${selected.length} records to the recycle bin?`)) return;
		const r = await api.post<{ done: number; failed: { error: string }[] }>(`/crm/${code}/bulk`, { ids: selected, action, value });
		r.failed.length ? toasts.error(new Error(`${r.done} done, ${r.failed.length} failed: ${r.failed[0]!.error}`)) : toasts.success(`${r.done} updated`);
		selected = [];
		load();
	}

	async function exportCsv() {
		try {
			await downloadUrl(`/api/crm/${code}/export` + params(), `${code}-${new Date().toISOString().slice(0, 10)}.csv`);
		} catch (e) {
			toasts.error(e);
		}
	}

	const listFields = $derived(type?.custom_fields.filter((d) => d.show_in_list) ?? []);
	const activeFilters = $derived(Object.keys(f).filter((k) => k !== 'category_id').length);
</script>

<svelte:head><title>{type?.name_plural ?? 'CRM'} · NERP</title></svelte:head>

{#if type}
	<div class="space-y-4">
		<div class="flex flex-wrap items-center gap-3">
			<h1 class="text-2xl font-semibold text-slate-900">{type.name_plural}</h1>
			{#if type.has_pipelines && type.categories.length > 1}
				<select class="input w-48" value={category?.id} onchange={(e) => setF('category_id', e.currentTarget.value)} aria-label="Pipeline">
					{#each type.categories as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
				</select>
			{/if}
			<div class="ml-auto flex flex-wrap gap-2">
				{#if type.rights.import !== 'none'}<a href="/crm/{code}/import" class="btn btn-secondary"><Icon name="upload" class="size-4" /> Import</a>{/if}
				{#if type.rights.export !== 'none'}<button class="btn btn-secondary" onclick={exportCsv}><Icon name="download" class="size-4" /> Export</button>{/if}
				{#if type.rights.add !== 'none'}<a href="/crm/{code}/new{f.category_id ? `?category_id=${f.category_id}` : ''}" class="btn btn-primary"><Icon name="plus" class="size-4" /> New {type.name.toLowerCase()}</a>{/if}
			</div>
		</div>

		<!-- Filter bar -->
		<div class="card flex flex-wrap items-center gap-2 p-2">
			<div class="relative min-w-56 flex-1">
				<Icon name="search" class="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-slate-400" />
				<input class="input border-0 pl-9 shadow-none focus:ring-0" placeholder="Search name, phone, email or #number" bind:value={q} oninput={onSearch} />
			</div>
			<button class="btn {f.assigned_to === 'me' ? 'bg-brand-100 text-brand-900' : 'btn-ghost'}" onclick={() => setF('assigned_to', f.assigned_to === 'me' ? null : 'me')}>Mine</button>
			{#if type.has_stages}
				<select class="input w-36" value={f.semantics ?? ''} onchange={(e) => setF('semantics', e.currentTarget.value)} aria-label="State">
					<option value="">Any state</option>
					<option value="process">In progress</option>
					<option value="success">{type.code === 'lead' ? 'Converted' : 'Won'}</option>
					<option value="failure">{type.code === 'lead' ? 'Junk' : 'Lost'}</option>
				</select>
			{/if}
			<select class="input w-40" value={f.activity ?? ''} onchange={(e) => setF('activity', e.currentTarget.value)} aria-label="Activities">
				<option value="">Any activity</option>
				<option value="overdue">Overdue</option>
				<option value="today">Due today</option>
				<option value="planned">Has planned</option>
				<option value="none">Nothing planned</option>
			</select>
			<button class="btn btn-ghost" onclick={() => (showFilters = !showFilters)}>
				<Icon name="settings" class="size-4" /> Filters{activeFilters ? ` (${activeFilters})` : ''}
			</button>
			{#if saved.length}
				<select class="input w-40" onchange={(e) => { const s = saved.find((x) => x.id === e.currentTarget.value); if (s) applySaved(s); e.currentTarget.value = ''; }} aria-label="Saved filters">
					<option value="">Saved filters…</option>
					{#each saved as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
				</select>
			{/if}
			{#if type.has_stages}
				<div class="flex rounded-full border border-slate-300 p-0.5">
					<button class="rounded-full p-1.5 {view === 'kanban' ? 'bg-brand-100 text-brand-900' : 'text-slate-400'}" aria-label="Board" onclick={() => setView('kanban')}><Icon name="kanban" class="size-4" /></button>
					<button class="rounded-full p-1.5 {view === 'list' ? 'bg-brand-100 text-brand-900' : 'text-slate-400'}" aria-label="List" onclick={() => setView('list')}><Icon name="list" class="size-4" /></button>
				</div>
			{/if}
		</div>

		{#if showFilters}
			<div class="card grid gap-3 p-4 sm:grid-cols-2 lg:grid-cols-4">
				<div><span class="label">Responsible</span><UserPicker bind:value={assignedId} bind:label={assignedLabel} onpick={(u) => setF('assigned_to', u.id)} /></div>
				<div>
					<label class="label" for="f-source">Source</label>
					<select id="f-source" class="input" value={f.source ?? ''} onchange={(e) => setF('source', e.currentTarget.value)}>
						<option value="">Any</option>
						{#each crm.config?.options.source ?? [] as o (o)}<option value={o}>{optionLabel(o)}</option>{/each}
					</select>
				</div>
				<div><label class="label" for="f-cf">Created from</label><input id="f-cf" type="date" class="input" value={f.created_from ?? ''} onchange={(e) => setF('created_from', e.currentTarget.value)} /></div>
				<div><label class="label" for="f-ct">Created to</label><input id="f-ct" type="date" class="input" value={f.created_to ?? ''} onchange={(e) => setF('created_to', e.currentTarget.value)} /></div>
				{#if type.has_amount}
					<div><label class="label" for="f-amin">Amount from ({base})</label><input id="f-amin" type="number" class="input" value={f.amount_min ?? ''} onchange={(e) => setF('amount_min', e.currentTarget.value)} /></div>
					<div><label class="label" for="f-amax">Amount to ({base})</label><input id="f-amax" type="number" class="input" value={f.amount_max ?? ''} onchange={(e) => setF('amount_max', e.currentTarget.value)} /></div>
				{/if}
				{#if type.code === 'deal'}
					<div><label class="label" for="f-close">Closing by</label><input id="f-close" type="date" class="input" value={f.close_to ?? ''} onchange={(e) => setF('close_to', e.currentTarget.value)} /></div>
				{/if}
				<div class="flex items-end gap-2">
					<button class="btn btn-secondary" onclick={saveFilter}>Save filter</button>
					<button class="btn btn-ghost" onclick={() => { f = {}; q = ''; assignedId = null; assignedLabel = null; load(); }}>Reset</button>
				</div>
			</div>
		{/if}

		{#if view === 'kanban' && type.has_stages}
			<!-- Kanban -->
			<div class="flex gap-3 overflow-x-auto pb-4 {loading ? 'opacity-70' : ''}">
				{#each columns as col (col.stage.id)}
					<section
						class="flex w-72 shrink-0 flex-col rounded-2xl bg-surface-container {overStage === col.stage.id ? 'ring-2 ring-brand-400' : ''}"
						role="list"
						ondragover={(e) => (e.preventDefault(), (overStage = col.stage.id))}
						ondragleave={() => (overStage = null)}
						ondrop={(e) => (e.preventDefault(), drop(col.stage))}
					>
						<header class="rounded-t-2xl px-3 pt-2.5 pb-2" style="border-top: 4px solid {col.stage.color}">
							<div class="flex items-center gap-2">
								<h3 class="truncate text-sm font-semibold text-slate-800">{col.stage.name}</h3>
								<span class="ml-auto text-xs text-slate-500">{col.count}{col.count_is_capped ? '+' : ''}</span>
							</div>
							{#if type.has_amount}<p class="text-xs text-slate-500">{moneyShort(col.sum_base, base)}</p>{/if}
						</header>
						<div class="flex-1 space-y-2 overflow-y-auto px-2 pb-2" style="max-height: calc(100vh - 300px)">
							{#each col.items as r (r.id)}
								<a
									href="/crm/{code}/{r.id}"
									draggable="true"
									ondragstart={() => (dragging = r)}
									ondragend={() => ((dragging = null), (overStage = null))}
									class="block rounded-xl bg-white p-3 shadow-g1 transition-shadow hover:shadow-g2 {dragging?.id === r.id ? 'opacity-40' : ''}"
								>
									<p class="text-sm font-medium text-slate-900">{r.title}</p>
									{#if type.has_amount && Number(r.opportunity)}<p class="mt-0.5 text-sm font-medium text-slate-700">{money(r.opportunity, r.currency)}</p>{/if}
									{#if r.company_title}<p class="truncate text-xs text-slate-500">{r.company_title}</p>{/if}
									{#if invoiceOverdue(type, r)}
										<p class="mt-1 inline-flex items-center gap-1 rounded bg-red-50 px-1.5 py-0.5 text-xs font-medium text-red-700">Overdue since {new Date(r.close_date + 'T00:00').toLocaleDateString()}</p>
									{/if}
									{#if overLimit(stageOf(col.stage.id), r.stage_changed_at, now) !== null}
										<p class="mt-1 inline-flex items-center gap-1 rounded bg-red-50 px-1.5 py-0.5 text-xs font-medium text-red-700" title="Time limit in this stage: {durationText(stageOf(col.stage.id)?.sla_minutes ?? 0)}">
											<Icon name="clock" class="size-3" /> {durationText(overLimit(stageOf(col.stage.id), r.stage_changed_at, now) ?? 0)} over the limit
										</p>
									{/if}
									<div class="mt-2 flex items-center gap-2 text-xs text-slate-400">
										{#if r.assigned_name}<Avatar name={r.assigned_name} fileId={r.assigned_avatar_file_id} size="xs" />{/if}
										{#if r.next_activity_at}
											<span class={new Date(r.next_activity_at) < new Date() ? 'rounded bg-red-50 px-1.5 py-0.5 font-medium text-red-600' : ''}>
												<Icon name="clock" class="inline size-3" /> {new Date(r.next_activity_at) < new Date() ? `${formatRelative(r.next_activity_at).replace(' ago', '')} overdue` : new Date(r.next_activity_at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}
											</span>
										{:else if !r.is_closed}
											<span class="text-amber-600">No activity planned</span>
										{/if}
									</div>
								</a>
							{/each}
							{#if col.items.length < col.count}
								<button class="w-full rounded-lg py-1.5 text-xs font-medium text-brand-600 hover:bg-white" onclick={() => more(col)}>Show more</button>
							{/if}
						</div>
					</section>
				{/each}
			</div>
		{:else}
			<!-- List -->
			{#if selected.length}
				<div class="card flex flex-wrap items-center gap-2 bg-brand-50 p-2 text-sm">
					<span class="px-2 font-medium text-brand-900">{selected.length} selected</span>
					<div class="w-56"><UserPicker bind:value={bulkUser} bind:label={bulkUserLabel} placeholder="Assign to…" onpick={(u) => bulk('assign', u.id)} /></div>
					{#if type.has_stages && category}
						<select class="input w-44" onchange={(e) => e.currentTarget.value && bulk('stage', e.currentTarget.value)} aria-label="Move to stage">
							<option value="">Move to stage…</option>
							{#each category.stages as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
						</select>
					{/if}
					<button class="btn btn-ghost text-red-600" onclick={() => bulk('delete')}><Icon name="trash" class="size-4" /> Delete</button>
					<button class="btn btn-ghost ml-auto" onclick={() => (selected = [])}>Clear</button>
				</div>
			{/if}
			<div class="card overflow-x-auto">
				<table class="min-w-full text-sm {loading ? 'opacity-60' : ''}">
					<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500">
						<tr>
							<th class="w-10 px-3 py-3"><input type="checkbox" class="rounded border-slate-300 text-brand-600" aria-label="Select all" checked={rows.length > 0 && selected.length === rows.length} onchange={(e) => (selected = e.currentTarget.checked ? rows.map((r) => r.id) : [])} /></th>
							<th class="px-3 py-3">{type.name}</th>
							{#if type.has_stages}<th class="px-3 py-3">Stage</th>{/if}
							{#if type.has_amount}<th class="px-3 py-3 text-right">Amount</th>{/if}
							{#if linksCompany(type)}<th class="hidden px-3 py-3 lg:table-cell">Company</th>{/if}
							<th class="hidden px-3 py-3 md:table-cell">Contact</th>
							{#each listFields as d (d.id)}<th class="hidden px-3 py-3 xl:table-cell">{d.label}</th>{/each}
							<th class="hidden px-3 py-3 md:table-cell">Responsible</th>
							<th class="hidden px-3 py-3 lg:table-cell">Activity</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-50">
						{#each rows as r (r.id)}
							<tr class="hover:bg-slate-50">
								<td class="px-3 py-2.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" aria-label="Select" bind:group={selected} value={r.id} /></td>
								<td class="px-3 py-2.5">
									<a href="/crm/{code}/{r.id}" class="font-medium text-slate-900 hover:text-brand-700">{r.title}</a>
									<span class="block text-xs text-slate-400">#{r.number}{r.job_title ? ` · ${r.job_title}` : ''}</span>
								</td>
								{#if type.has_stages}<td class="px-3 py-2.5"><span class="badge text-white" style="background: {r.stage_color}">{r.stage_name}</span></td>{/if}
								{#if type.has_amount}<td class="px-3 py-2.5 text-right whitespace-nowrap">{money(r.opportunity, r.currency)}</td>{/if}
								{#if linksCompany(type)}<td class="hidden px-3 py-2.5 text-slate-600 lg:table-cell">{r.company_title ?? ''}</td>{/if}
								<td class="hidden px-3 py-2.5 text-slate-600 md:table-cell"><div>{r.phone ?? ''}</div><div class="text-xs text-slate-400">{r.email ?? ''}</div></td>
								{#each listFields as d (d.id)}<td class="hidden px-3 py-2.5 text-slate-600 xl:table-cell">{ufText(d, r.uf[d.code])}</td>{/each}
								<td class="hidden px-3 py-2.5 md:table-cell">{#if r.assigned_name}<span class="flex items-center gap-2"><Avatar name={r.assigned_name} fileId={r.assigned_avatar_file_id} size="xs" />{r.assigned_name}</span>{/if}</td>
								<td class="hidden px-3 py-2.5 text-xs lg:table-cell">
									{#if r.next_activity_at}<span class={new Date(r.next_activity_at) < new Date() ? 'font-medium text-red-600' : 'text-slate-500'}>{new Date(r.next_activity_at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}</span>{:else}<span class="text-slate-300">—</span>{/if}
								</td>
							</tr>
						{:else}
							<tr><td colspan="9" class="px-3 py-14 text-center text-slate-500">{loading ? 'Loading…' : `No ${type.name_plural.toLowerCase()} found.`}</td></tr>
						{/each}
					</tbody>
				</table>
				{#if total > PAGE}
					<div class="flex items-center justify-between border-t border-slate-100 px-4 py-3 text-sm text-slate-500">
						<span>{offset + 1}–{Math.min(offset + PAGE, total)} of {total}{capped ? '+' : ''}</span>
						<div class="flex gap-2">
							<button class="btn btn-secondary" disabled={offset === 0} onclick={() => ((offset -= PAGE), load())}>Previous</button>
							<button class="btn btn-secondary" disabled={offset + PAGE >= total && !capped} onclick={() => ((offset += PAGE), load())}>Next</button>
						</div>
					</div>
				{/if}
			</div>
		{/if}
	</div>
{:else if crm.config}
	<div class="card p-10 text-center text-slate-500">Unknown CRM section.</div>
{/if}
