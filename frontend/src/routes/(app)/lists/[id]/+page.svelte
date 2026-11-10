<script lang="ts">
	// One list: its items (with their processes), and for managers its fields, access and
	// business processes.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, downloadUrl } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import KbAccess from '#lib/components/KbAccess.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import CrmPicker from '#lib/components/crm/CrmPicker.svelte';
	import type { ListField } from '#lib/components/WfSteps.svelte';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Person = { id: string; name: string; avatar_file_id: string | null };
	type WhoSet = { users: Person[]; departments: string[] };
	type L = { id: string; name: string; description: string | null; to_all: boolean; level: 'read' | 'add' | 'manage'; fields: ListField[]; workflows: { id: string; name: string; trigger: string; is_active: boolean }[]; access?: { read: WhoSet; add: WhoSet; manage: WhoSet } };
	type Item = { id: string; number: number; title: string; values: Record<string, unknown>; author_name: string | null; created_at: string; process: string | null; process_status: string | null };
	type Process = { id: string; name: string; status: string; started_at: string; error: string | null; log: { text: string; user: string | null; at: string }[]; approvals: { id: string; title: string; status: string }[] };
	type Detail = Item & { can_edit: boolean; processes: Process[]; labels: Record<string, string>; created_by: string | null };

	const id = $derived(page.params.id ?? '');
	let l = $state<L | null>(null);
	let items = $state<Item[]>([]);
	let total = $state(0);
	let q = $state('');
	let mine = $state(false);
	let tab = $state<'items' | 'settings'>((page.url.searchParams.get('tab') as 'settings') ?? 'items');
	async function load() {
		l = await api.get<L>(`/lists/${id}`);
		await loadItems();
	}
	async function loadItems() {
		const r = await api.get<{ items: Item[]; total: number }>(`/lists/${id}/items?limit=300${q.trim() ? `&q=${encodeURIComponent(q.trim())}` : ''}${mine ? '&mine=true' : ''}`);
		items = r.items;
		total = r.total;
	}
	$effect(() => {
		id;
		load().then(() => {
			const it = page.url.searchParams.get('item');
			if (it) openItem(it);
		}).catch((e) => toasts.error(e));
	});
	let t: ReturnType<typeof setTimeout> | undefined;
	const search = () => (clearTimeout(t), (t = setTimeout(loadItems, 250)));
	const shown = $derived(l?.fields.filter((f) => !['textarea', 'file', 'crm', 'user'].includes(f.kind)).slice(0, 4) ?? []);
	const STATUS: Record<string, [string, string]> = { running: ['Running', 'bg-sky-50 text-sky-800'], waiting: ['Waiting', 'bg-amber-50 text-amber-800'], completed: ['Completed', 'bg-emerald-50 text-emerald-800'], rejected: ['Rejected', 'bg-red-50 text-red-700'], cancelled: ['Cancelled', 'bg-slate-100 text-slate-500'], failed: ['Failed', 'bg-red-50 text-red-700'] };
	const show = (v: unknown) => (v === true ? '✓' : v === false || v === undefined || v === null ? '' : String(v));

	// An item: view and edit.
	let itemOpen = $state(false);
	let detail = $state<Detail | null>(null);
	let form = $state<{ title: string; values: Record<string, unknown> }>({ title: '', values: {} });
	let labels = $state<Record<string, string>>({});
	let editing = $state(false);
	async function openItem(itemId: string | null) {
		detail = null;
		labels = {};
		if (itemId) {
			detail = await api.get<Detail>(`/lists/items/${itemId}`);
			form = { title: detail.title, values: { ...detail.values } };
			labels = { ...detail.labels };
			editing = false;
		} else {
			form = { title: '', values: {} };
			editing = true;
		}
		itemOpen = true;
	}
	async function saveItem(e: SubmitEvent) {
		e.preventDefault();
		const values: Record<string, unknown> = {};
		for (const f of l!.fields) {
			const v = form.values[f.code];
			values[f.code] = v === '' || v === undefined ? null : f.kind === 'number' && typeof v === 'number' ? v : f.kind === 'checkbox' ? !!v : v;
		}
		try {
			detail = detail ? await api.patch<Detail>(`/lists/items/${detail.id}`, { title: form.title, values }) : await api.post<Detail>(`/lists/${id}/items`, { title: form.title, values });
			form = { title: detail!.title, values: { ...detail!.values } };
			labels = { ...detail!.labels };
			editing = false;
			loadItems();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeItem() {
		if (!confirm(`Delete #${detail!.number} “${detail!.title}”?`)) return;
		await api.del(`/lists/items/${detail!.id}`);
		itemOpen = false;
		loadItems();
	}
	async function startProcess(wf: string) {
		try {
			detail!.processes = await api.post(`/lists/items/${detail!.id}/start/${wf}`, {});
			loadItems();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function cancelProcess(p: Process) {
		if (!confirm(`Cancel “${p.name}”?`)) return;
		detail!.processes = await api.post(`/workflows/instances/${p.id}/cancel`, {});
		loadItems();
	}
	async function upload(e: Event, code: string) {
		const f = (e.currentTarget as HTMLInputElement).files?.[0];
		if (!f) return;
		const fd = new FormData();
		fd.append('file', f);
		const up = await api.post<{ id: string }>('/files', fd);
		form.values[code] = up.id;
		labels[code] = f.name;
	}

	// Settings.
	const KINDS: [string, string][] = [['text', 'Text'], ['textarea', 'Long text'], ['number', 'Number'], ['money', 'Amount of money'], ['date', 'Date'], ['select', 'Choice'], ['user', 'Employee'], ['checkbox', 'Yes / no'], ['file', 'File'], ['crm', 'CRM record']];
	let fieldsDraft = $state<(ListField & { optionsText: string })[]>([]);
	let access = $state({ to_all: true, read: { users: [] as Person[], departments: [] as string[] }, edit: { users: [] as Person[], departments: [] as string[] }, manage: { users: [] as Person[], departments: [] as string[] } });
	$effect(() => {
		if (!l || l.level !== 'manage') return;
		fieldsDraft = l.fields.map((f) => ({ ...f, optionsText: f.options.join(', ') }));
		access = { to_all: l.to_all, read: l.access!.read, edit: l.access!.add, manage: l.access!.manage };
	});
	async function saveFields() {
		try {
			l = await api.put<L>(`/lists/${id}/fields`, fieldsDraft.map((f) => ({ id: f.id, code: f.code, label: f.label, kind: f.kind, required: f.required, options: f.optionsText.split(',').map((o) => o.trim()).filter(Boolean) })));
			toasts.success('Fields saved');
		} catch (err) {
			toasts.error(err);
		}
	}
	async function saveAccess() {
		const who = (w: WhoSet) => ({ users: w.users.map((u) => u.id), departments: w.departments });
		try {
			l = await api.patch<L>(`/lists/${id}`, { to_all: access.to_all, access: { read: who(access.read), add: who(access.edit), manage: who(access.manage) } });
			toasts.success('Access saved');
		} catch (err) {
			toasts.error(err);
		}
	}
	async function newWorkflow() {
		const name = prompt('Name of the process', 'Approval');
		if (!name?.trim()) return;
		const w = await api.post<{ id: string }>(`/lists/${id}/workflows`, { name, trigger: 'create', steps: [] });
		goto(`/lists/workflow/${w.id}`);
	}
	async function removeList() {
		if (!confirm(`Delete the list “${l!.name}” with all its items?`)) return;
		await api.del(`/lists/${id}`);
		goto('/lists');
	}
	const TRIGGERS: Record<string, string> = { create: 'when an item is added', update: 'when an item changes', manual: 'when started by hand' };
</script>

<svelte:head><title>{l?.name ?? 'List'} · NERP</title></svelte:head>

<a href="/lists" class="mb-4 inline-block text-sm text-slate-500 hover:text-slate-800">← Lists</a>
{#if l}
	<div class="space-y-4">
		<div class="flex flex-wrap items-center gap-3">
			<div class="min-w-0 flex-1"><h1 class="text-2xl font-semibold text-slate-900">{l.name}</h1>{#if l.description}<p class="text-sm text-slate-500">{l.description}</p>{/if}</div>
			{#if l.level === 'manage'}
				<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist">
					{#each [['items', 'Items'], ['settings', 'Settings']] as [k, n] (k)}<button role="tab" aria-selected={tab === k} class="rounded-full px-4 py-1.5 font-medium {tab === k ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = k as typeof tab)}>{n}</button>{/each}
				</div>
			{/if}
			{#if tab === 'items' && l.level !== 'read'}<button class="btn btn-primary" onclick={() => openItem(null)}><Icon name="plus" class="size-4" /> New item</button>{/if}
		</div>

		{#if tab === 'items'}
			<div class="flex flex-wrap items-center gap-3">
				<input class="input max-w-xs" type="search" placeholder="Search" aria-label="Search items" bind:value={q} oninput={search} />
				<label class="flex items-center gap-2 text-sm text-slate-600"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={mine} onchange={loadItems} /> Mine</label>
				<span class="text-sm text-slate-500">{total} item{total === 1 ? '' : 's'}</span>
				<button class="btn btn-ghost ml-auto text-sm" onclick={() => downloadUrl(`/api/lists/${id}/export`, `${l!.name}.csv`).catch(toasts.error)}><Icon name="download" class="size-4" /> CSV</button>
			</div>
			<div class="card overflow-x-auto">
				<table class="min-w-full text-sm">
					<thead class="border-b border-slate-100 text-left text-xs text-slate-500"><tr><th class="px-4 py-3">#</th><th class="px-4 py-3">Title</th>{#each shown as f (f.code)}<th class="px-4 py-3">{f.label}</th>{/each}<th class="px-4 py-3">Process</th><th class="px-4 py-3">Added</th></tr></thead>
					<tbody class="divide-y divide-slate-50">
						{#each items as it (it.id)}
							<tr class="cursor-pointer hover:bg-slate-50" data-item={it.title} onclick={() => openItem(it.id)}>
								<td class="px-4 py-2.5 text-slate-500 tabular-nums">{it.number}</td>
								<td class="px-4 py-2.5 font-medium text-slate-900">{it.title}</td>
								{#each shown as f (f.code)}<td class="px-4 py-2.5 text-slate-700">{show(it.values[f.code])}</td>{/each}
								<td class="px-4 py-2.5">{#if it.process_status}<span class="badge {STATUS[it.process_status]?.[1] ?? ''}" data-process-status={it.process_status}>{STATUS[it.process_status]?.[0] ?? it.process_status}</span>{/if}</td>
								<td class="px-4 py-2.5 whitespace-nowrap text-slate-500">{it.author_name ?? ''} · {formatRelative(it.created_at)}</td>
							</tr>
						{:else}
							<tr><td colspan={shown.length + 4} class="px-4 py-12 text-center text-slate-500">No items yet.</td></tr>
						{/each}
					</tbody>
				</table>
			</div>
		{:else}
			<div class="grid gap-5 lg:grid-cols-2">
				<section class="card space-y-3 p-5" aria-label="Fields">
					<h2 class="font-medium text-slate-900">Fields</h2>
					{#each fieldsDraft as f, i (i)}
						<div class="space-y-2 rounded-xl border border-slate-200 p-3" data-field={f.label}>
							<div class="flex gap-2">
								<input class="input flex-1" placeholder="Name" aria-label="Field name" bind:value={f.label} />
								<select class="input w-40" aria-label="Kind" disabled={!!f.id} bind:value={f.kind}>{#each KINDS as [k, n] (k)}<option value={k}>{n}</option>{/each}</select>
								<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Remove field" onclick={() => fieldsDraft.splice(i, 1)}><Icon name="trash" class="size-4" /></button>
							</div>
							{#if f.kind === 'select'}<input class="input text-sm" placeholder="Choices, separated by commas" aria-label="Choices" bind:value={f.optionsText} />{/if}
							<label class="flex items-center gap-2 text-sm text-slate-600"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={f.required} /> Required</label>
						</div>
					{/each}
					<div class="flex gap-2"><button class="btn btn-secondary" onclick={() => fieldsDraft.push({ code: '', label: '', kind: 'text', options: [], required: false, optionsText: '' })}>Add field</button><button class="btn btn-primary ml-auto" onclick={saveFields}>Save fields</button></div>
				</section>
				<div class="space-y-5">
					<section class="card space-y-3 p-5" aria-label="Processes">
						<h2 class="font-medium text-slate-900">Business processes</h2>
						{#each l.workflows as w (w.id)}<a href="/lists/workflow/{w.id}" class="flex items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-slate-50" data-workflow={w.name}><span class="flex-1 font-medium">{w.name}</span><span class="text-xs text-slate-500">starts {TRIGGERS[w.trigger]}</span>{#if !w.is_active}<span class="badge bg-slate-100">Off</span>{/if}</a>{:else}<p class="text-sm text-slate-500">None yet: add one to route items for approval, notify people or create tasks.</p>{/each}
						<button class="btn btn-secondary" onclick={newWorkflow}>New process</button>
					</section>
					<section class="card space-y-3 p-5" aria-label="Access">
						<h2 class="font-medium text-slate-900">Who can use it</h2>
						<KbAccess bind:toAll={access.to_all} bind:readers={access.read} bind:editors={access.edit} hint="They add items and change their own. Whoever made the list and the people below manage it." />
						<div><span class="label">Who manages it</span><MultiUserPicker bind:value={access.manage.users} placeholder="People…" /></div>
						<p class="text-xs text-slate-500">“Who reads it” sees items; “Who edits it” also adds items and changes their own. With “Everyone”, everyone adds items.</p>
						<button class="btn btn-primary" onclick={saveAccess}>Save access</button>
					</section>
					<button class="text-sm text-slate-400 hover:text-red-600" onclick={removeList}>Delete this list</button>
				</div>
			</div>
		{/if}
	</div>
{/if}

<Modal bind:open={itemOpen} title={detail ? `#${detail.number} ${detail.title}` : 'New item'}>
	{#if l}
		{#if editing}
			<form id="item-form" class="space-y-3" onsubmit={saveItem}>
				<div><label class="label" for="it-title">Title</label><input id="it-title" class="input" required maxlength="300" bind:value={form.title} /></div>
				{#each l.fields as f (f.code)}
					<div>
						<label class="label" for="it-{f.code}">{f.label}{f.required ? ' *' : ''}</label>
						{#if f.kind === 'textarea'}<textarea id="it-{f.code}" class="input" rows="3" bind:value={form.values[f.code]}></textarea>
						{:else if f.kind === 'number' || f.kind === 'money'}<input id="it-{f.code}" class="input" type="number" step="any" bind:value={form.values[f.code]} />
						{:else if f.kind === 'date'}<input id="it-{f.code}" class="input" type="date" bind:value={form.values[f.code]} />
						{:else if f.kind === 'select'}<select id="it-{f.code}" class="input" bind:value={form.values[f.code]}><option value={null}>—</option>{#each f.options as o (o)}<option>{o}</option>{/each}</select>
						{:else if f.kind === 'checkbox'}<input id="it-{f.code}" type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.values[f.code] as boolean} />
						{:else if f.kind === 'user'}
							{#if form.values[f.code]}<p class="text-sm">{labels[f.code] ?? '…'} <button type="button" class="ml-2 text-slate-400 hover:text-red-600" onclick={() => (form.values[f.code] = null)}>change</button></p>
							{:else}<UserPicker onpick={(u) => ((form.values[f.code] = u.id), (labels[f.code] = u.full_name))} />{/if}
						{:else if f.kind === 'crm'}
							{#if form.values[f.code]}<p class="text-sm">{labels[f.code] ?? '…'} <button type="button" class="ml-2 text-slate-400 hover:text-red-600" onclick={() => (form.values[f.code] = null)}>change</button></p>
							{:else}<CrmPicker types={['deal', 'lead', 'contact', 'company']} onpick={(h) => ((form.values[f.code] = h.id), (labels[f.code] = h.title))} />{/if}
						{:else if f.kind === 'file'}
							{#if form.values[f.code]}<p class="text-sm">{labels[f.code] ?? 'File'} <button type="button" class="ml-2 text-slate-400 hover:text-red-600" onclick={() => (form.values[f.code] = null)}>remove</button></p>
							{:else}<input id="it-{f.code}" type="file" class="text-sm" onchange={(e) => upload(e, f.code)} />{/if}
						{:else}<input id="it-{f.code}" class="input" bind:value={form.values[f.code]} />{/if}
					</div>
				{/each}
			</form>
		{:else if detail}
			<dl class="grid grid-cols-[10rem_1fr] gap-x-4 gap-y-2 text-sm" data-item-view>
				{#each l.fields as f (f.code)}<dt class="text-slate-500">{f.label}</dt><dd class="text-slate-900">{(labels[f.code]?.replace(/^\w+:/, '') ?? show(detail.values[f.code])) || '—'}</dd>{/each}
				<dt class="text-slate-500">Added</dt><dd>{detail.author_name ?? ''} · {formatDateTime(detail.created_at)}</dd>
			</dl>
			<section class="mt-5 space-y-3" aria-label="Processes">
				{#each detail.processes as p (p.id)}
					<div class="rounded-xl border border-slate-200 p-3" data-process={p.name}>
						<div class="flex items-center gap-2"><span class="flex-1 font-medium">{p.name}</span><span class="badge {STATUS[p.status]?.[1]}">{STATUS[p.status]?.[0] ?? p.status}</span>{#if p.status === 'running' || p.status === 'waiting'}<button class="text-xs text-slate-400 hover:text-red-600" onclick={() => cancelProcess(p)}>Cancel</button>{/if}</div>
						{#if p.error}<p class="text-xs text-red-700">{p.error}</p>{/if}
						<ol class="mt-2 space-y-0.5 text-xs text-slate-600">{#each p.log as e, i (i)}<li><span class="text-slate-400">{formatDateTime(e.at)}</span> {e.text}</li>{/each}</ol>
					</div>
				{/each}
				{#if detail.can_edit && l.workflows.some((w) => w.trigger === 'manual' && w.is_active)}
					<div class="flex flex-wrap gap-2">{#each l.workflows.filter((w) => w.trigger === 'manual' && w.is_active) as w (w.id)}<button class="btn btn-secondary px-3 py-1 text-sm" onclick={() => startProcess(w.id)}>Start “{w.name}”</button>{/each}</div>
				{/if}
			</section>
		{/if}
	{/if}
	{#snippet footer()}
		{#if editing}
			<button class="btn btn-secondary" onclick={() => (detail ? (editing = false) : (itemOpen = false))}>Cancel</button>
			<button class="btn btn-primary" form="item-form">Save</button>
		{:else if detail}
			{#if detail.can_edit}<button class="btn btn-ghost mr-auto hover:text-red-600" onclick={removeItem}>Delete</button><button class="btn btn-secondary" onclick={() => (editing = true)}>Edit</button>{/if}
			<button class="btn btn-primary" onclick={() => (itemOpen = false)}>Close</button>
		{/if}
	{/snippet}
</Modal>
