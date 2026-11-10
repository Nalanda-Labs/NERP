<script lang="ts">
	// Marketing → Segments: saved audiences of leads, contacts or companies, with how many of
	// them can be reached (have an email address and have not unsubscribed).
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { crm, optionLabel } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Filter = { q?: string; source?: string; semantics?: string; created_from?: string; created_to?: string; assigned_to?: string | null };
	type Segment = { id: string; name: string; entity_type: number; type_code: string; filter: Filter; count: number; with_email: number; opted_out: number; reachable: number };
	type Preview = { count: number; with_email: number; opted_out: number; reachable: number; sample: { id: string; title: string; email: string | null }[]; type_code: string };

	const TYPES: [number, string][] = [[3, 'Contacts'], [4, 'Companies'], [1, 'Leads']];
	let items = $state<Segment[]>([]);
	let loaded = $state(false);
	async function load() {
		try {
			items = (await api.get<{ items: Segment[] }>('/marketing/segments')).items;
			loaded = true;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(() => {
		crm.load();
		load();
	});

	let open = $state(false);
	let draft = $state<{ id: string | null; name: string; entity_type: number; filter: Filter; assignedLabel: string | null }>({ id: null, name: '', entity_type: 3, filter: {}, assignedLabel: null });
	let preview = $state<Preview | null>(null);
	let previewError = $state<string | null>(null);
	let saving = $state(false);
	function edit(s: Segment | null) {
		draft = s
			? { id: s.id, name: s.name, entity_type: s.entity_type, filter: { ...s.filter, assigned_to: s.filter.assigned_to ?? null }, assignedLabel: s.filter.assigned_to ? '…' : null }
			: { id: null, name: '', entity_type: 3, filter: { assigned_to: null }, assignedLabel: null };
		preview = null;
		open = true;
	}
	/** The filter without empty values. */
	const clean = (f: Filter) => Object.fromEntries(Object.entries(f).filter(([, v]) => v !== undefined && v !== null && v !== ''));
	let timer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => {
		if (!open) return;
		const body = { entity_type: draft.entity_type, filter: clean(draft.filter) };
		clearTimeout(timer);
		timer = setTimeout(async () => {
			try {
				preview = await api.post<Preview>('/marketing/segments/preview', body);
				previewError = null;
			} catch (e) {
				preview = null;
				previewError = e instanceof Error ? e.message : 'Cannot count with these filters';
			}
		}, 300);
	});
	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			const body = { name: draft.name, entity_type: draft.entity_type, filter: clean(draft.filter) };
			if (draft.id) await api.patch(`/marketing/segments/${draft.id}`, body);
			else await api.post('/marketing/segments', body);
			open = false;
			toasts.success('Segment saved');
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
	async function remove() {
		if (!draft.id || !confirm(`Delete “${draft.name}”?`)) return;
		try {
			await api.del(`/marketing/segments/${draft.id}`);
			open = false;
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	const typeLabel = (id: number) => TYPES.find((t) => t[0] === id)?.[1] ?? '';
	const summary = (f: Filter) =>
		[f.q && `“${f.q}”`, f.source && optionLabel(f.source), f.semantics && { process: 'open', success: 'converted', failure: 'junk' }[f.semantics], f.created_from && `from ${f.created_from}`, f.created_to && `to ${f.created_to}`, f.assigned_to && 'one responsible']
			.filter(Boolean)
			.join(' · ') || 'everyone';
</script>

<svelte:head><title>Segments · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-end gap-3">
		<div class="flex-1">
			<h1 class="text-2xl font-semibold text-slate-900">Segments</h1>
			<p class="mt-1 text-sm text-slate-500">Saved audiences for campaigns. They're worked out again each time a campaign is sent, so new records join automatically.</p>
		</div>
		<button class="btn btn-primary" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> New segment</button>
	</div>

	{#if items.length}
		<div class="card divide-y divide-slate-100">
			{#each items as s (s.id)}
				<button class="flex w-full flex-wrap items-center gap-4 px-5 py-3 text-left text-sm hover:bg-slate-50" onclick={() => edit(s)} data-segment={s.id}>
					<div class="min-w-0 flex-1">
						<p class="font-medium text-slate-900">{s.name}</p>
						<p class="text-xs text-slate-500">{typeLabel(s.entity_type)} · {summary(s.filter)}</p>
					</div>
					<div class="text-right">
						<p class="text-lg font-semibold text-slate-900 tabular-nums">{s.reachable}</p>
						<p class="text-xs text-slate-500">can be reached of {s.count}</p>
					</div>
				</button>
			{/each}
		</div>
	{:else if loaded}
		<div class="card p-8 text-center text-sm text-slate-500">No segments yet.</div>
	{/if}
</div>

<Modal bind:open title={draft.id ? 'Edit segment' : 'New segment'} size="lg">
	<form id="seg-form" class="grid gap-5 text-sm md:grid-cols-[1fr_15rem]" onsubmit={save}>
		<div class="space-y-3">
			<div><label class="label" for="sg-name">Name</label><input id="sg-name" class="input" required maxlength="120" bind:value={draft.name} placeholder="e.g. Customers in Germany" /></div>
			<div>
				<span class="label">Made of</span>
				<div class="flex gap-1.5" role="radiogroup" aria-label="Record type">
					{#each TYPES as [id, label] (id)}
						<button type="button" role="radio" aria-checked={draft.entity_type === id} class="rounded-full px-3 py-1 {draft.entity_type === id ? 'bg-brand-100 font-medium text-brand-900' : 'bg-slate-100 text-slate-600 hover:bg-slate-200'}" onclick={() => ((draft.entity_type = id), (draft.filter.semantics = undefined))}>{label}</button>
					{/each}
				</div>
			</div>
			<div><label class="label" for="sg-q">Name, email or phone contains</label><input id="sg-q" class="input" bind:value={draft.filter.q} placeholder="at least 3 characters" /></div>
			<div class="grid gap-3 sm:grid-cols-2">
				<div>
					<label class="label" for="sg-src">Source</label>
					<select id="sg-src" class="input" bind:value={draft.filter.source}>
						<option value="">Any</option>
						{#each crm.config?.options.source ?? [] as o (o)}<option value={o}>{optionLabel(o)}</option>{/each}
					</select>
				</div>
				{#if draft.entity_type === 1}
					<div>
						<label class="label" for="sg-sem">Lead status</label>
						<select id="sg-sem" class="input" bind:value={draft.filter.semantics}><option value="">Any</option><option value="process">Open</option><option value="success">Converted</option><option value="failure">Junk</option></select>
					</div>
				{/if}
				<div><label class="label" for="sg-from">Created from</label><input id="sg-from" type="date" class="input" bind:value={draft.filter.created_from} /></div>
				<div><label class="label" for="sg-to">Created to</label><input id="sg-to" type="date" class="input" bind:value={draft.filter.created_to} /></div>
			</div>
			<div>
				<span class="label">Responsible</span>
				<UserPicker bind:value={draft.filter.assigned_to} bind:label={draft.assignedLabel} placeholder="Anyone" />
			</div>
		</div>
		<aside class="rounded-2xl bg-slate-50 p-4" aria-live="polite" aria-label="Audience">
			{#if preview}
				<p class="text-3xl font-semibold text-slate-900 tabular-nums" data-reach>{preview.reachable}</p>
				<p class="text-xs text-slate-500">can be reached</p>
				<dl class="mt-3 space-y-1 text-xs text-slate-600">
					<div class="flex justify-between"><dt>Matching</dt><dd class="tabular-nums">{preview.count}</dd></div>
					<div class="flex justify-between"><dt>With an email address</dt><dd class="tabular-nums">{preview.with_email}</dd></div>
					<div class="flex justify-between"><dt>Unsubscribed</dt><dd class="tabular-nums">{preview.opted_out}</dd></div>
				</dl>
				{#if preview.sample.length}
					<ul class="mt-3 space-y-1 border-t border-slate-200 pt-3 text-xs">
						{#each preview.sample.slice(0, 8) as m (m.id)}
							<li class="truncate"><a class="text-slate-800 hover:underline" href="/crm/{preview.type_code}/{m.id}" target="_blank">{m.title}</a> <span class="text-slate-400">{m.email ?? 'no email'}</span></li>
						{/each}
					</ul>
				{/if}
			{:else if previewError}
				<p class="text-xs text-amber-800">{previewError}</p>
			{:else}
				<p class="text-xs text-slate-400">Counting…</p>
			{/if}
		</aside>
	</form>
	{#snippet footer()}
		{#if draft.id}<button class="btn btn-ghost mr-auto text-red-600" onclick={remove}><Icon name="trash" class="size-4" /> Delete</button>{/if}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" type="submit" form="seg-form" disabled={saving}>{saving ? 'Saving…' : 'Save'}</button>
	{/snippet}
</Modal>
