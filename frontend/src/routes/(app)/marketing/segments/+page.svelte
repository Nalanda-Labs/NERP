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
	type Segment = { id: string; name: string; entity_type: number; type_code: string; filter: Filter; imported: boolean; count: number; with_email: number; opted_out: number; reachable: number };
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
	let draft = $state<{ id: string | null; name: string; entity_type: number; filter: Filter; assignedLabel: string | null; imported?: Segment }>({ id: null, name: '', entity_type: 3, filter: {}, assignedLabel: null });
	let preview = $state<Preview | null>(null);
	let previewError = $state<string | null>(null);
	let saving = $state(false);
	function edit(s: Segment | null) {
		draft = s
			? { id: s.id, name: s.name, entity_type: s.entity_type, filter: { ...s.filter, assigned_to: s.filter.assigned_to ?? null }, assignedLabel: s.filter.assigned_to ? '…' : null, imported: s.imported ? s : undefined }
			: { id: null, name: '', entity_type: 3, filter: { assigned_to: null }, assignedLabel: null };
		preview = null;
		open = true;
	}
	/** The filter without empty values. */
	const clean = (f: Filter) => Object.fromEntries(Object.entries(f).filter(([, v]) => v !== undefined && v !== null && v !== ''));
	let timer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => {
		if (!open || draft.imported) return;
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
			const body = draft.imported ? { name: draft.name } : { name: draft.name, entity_type: draft.entity_type, filter: clean(draft.filter) };
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
	// ----- importing a list of addresses -----
	type Row = { email: string; first_name?: string; last_name?: string; company_name?: string };
	let imp = $state({ open: false, name: '', entity_type: 3, text: '', create_missing: true, busy: false });
	let impResult = $state<{ matched: number; created: number; not_found: number; invalid: number } | null>(null);
	/** One address per line; extra columns are first name, last name and company, or follow a header row. */
	function parseRows(text: string): Row[] {
		const lines = text.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
		if (!lines.length) return [];
		const split = (l: string) => l.split(/[,;\t]/).map((c) => c.trim().replace(/^"(.*)"$/, '$1'));
		const head = split(lines[0]).map((c) => c.toLowerCase().replace(/[^a-z]/g, ''));
		const at = (names: string[]) => head.findIndex((h) => names.includes(h));
		const hasHeader = at(['email', 'emailaddress', 'mail']) >= 0;
		const cols = hasHeader
			? { email: at(['email', 'emailaddress', 'mail']), first_name: at(['firstname', 'first', 'name']), last_name: at(['lastname', 'last', 'surname']), company_name: at(['company', 'companyname', 'organisation', 'organization']) }
			: null;
		return (hasHeader ? lines.slice(1) : lines).map((l) => {
			const c = split(l);
			if (cols) return { email: c[cols.email] ?? '', first_name: c[cols.first_name], last_name: c[cols.last_name], company_name: c[cols.company_name] };
			const i = c.findIndex((x) => x.includes('@'));
			const rest = c.filter((_, j) => j !== i);
			return { email: c[i] ?? '', first_name: rest[0], last_name: rest[1], company_name: rest[2] };
		});
	}
	const impRows = $derived(parseRows(imp.text));
	async function readFile(e: Event) {
		const f = (e.currentTarget as HTMLInputElement).files?.[0];
		if (f) imp.text = await f.text();
	}
	async function runImport(e: SubmitEvent) {
		e.preventDefault();
		imp.busy = true;
		try {
			const r = await api.post<{ import: typeof impResult }>('/marketing/segments/import', { name: imp.name, entity_type: imp.entity_type, rows: impRows, create_missing: imp.create_missing });
			impResult = r.import;
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			imp.busy = false;
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
		<button class="btn btn-secondary" onclick={() => ((imp = { open: true, name: '', entity_type: 3, text: '', create_missing: true, busy: false }), (impResult = null))}><Icon name="upload" class="size-4" /> Import</button>
		<button class="btn btn-primary" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> New segment</button>
	</div>

	{#if items.length}
		<div class="card divide-y divide-slate-100">
			{#each items as s (s.id)}
				<button class="flex w-full flex-wrap items-center gap-4 px-5 py-3 text-left text-sm hover:bg-slate-50" onclick={() => edit(s)} data-segment={s.id}>
					<div class="min-w-0 flex-1">
						<p class="font-medium text-slate-900">{s.name}</p>
						<p class="text-xs text-slate-500">{typeLabel(s.entity_type)} · {s.imported ? 'imported list' : summary(s.filter)}</p>
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
	{#if draft.imported}
		{@const g = draft.imported}
		<form id="seg-form" class="space-y-3 text-sm" onsubmit={save}>
			<div><label class="label" for="sg-name2">Name</label><input id="sg-name2" class="input" required maxlength="120" bind:value={draft.name} /></div>
			<p class="text-slate-600">An imported list of {typeLabel(g.entity_type).toLowerCase()}: {g.count} records, {g.reachable} can be reached ({g.opted_out} unsubscribed). It doesn't change when new records are added.</p>
		</form>
	{:else}
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
	{/if}
	{#snippet footer()}
		{#if draft.id}<button class="btn btn-ghost mr-auto text-red-600" onclick={remove}><Icon name="trash" class="size-4" /> Delete</button>{/if}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" type="submit" form="seg-form" disabled={saving}>{saving ? 'Saving…' : 'Save'}</button>
	{/snippet}
</Modal>

<Modal bind:open={imp.open} title="Import a segment" size="lg">
	{#if impResult}
		<div class="space-y-2 text-sm" data-import-result>
			<p class="font-medium text-slate-900">“{imp.name}” is ready.</p>
			<ul class="space-y-1 text-slate-600">
				<li>{impResult.matched} already in the CRM</li>
				{#if impResult.created}<li>{impResult.created} added as new {imp.entity_type === 1 ? 'leads' : 'contacts'}</li>{/if}
				{#if impResult.not_found}<li>{impResult.not_found} not in the CRM, left out</li>{/if}
				{#if impResult.invalid}<li>{impResult.invalid} not valid email addresses</li>{/if}
			</ul>
		</div>
	{:else}
		<form id="imp-form" class="space-y-3 text-sm" onsubmit={runImport}>
			<div><label class="label" for="im-name">Name</label><input id="im-name" class="input" required maxlength="120" bind:value={imp.name} placeholder="e.g. Trade fair visitors" /></div>
			<div>
				<span class="label">Match against</span>
				<div class="flex gap-1.5" role="radiogroup" aria-label="Record type">
					{#each [[3, 'Contacts'], [1, 'Leads']] as [id, label] (id)}
						<button type="button" role="radio" aria-checked={imp.entity_type === id} class="rounded-full px-3 py-1 {imp.entity_type === id ? 'bg-brand-100 font-medium text-brand-900' : 'bg-slate-100 text-slate-600 hover:bg-slate-200'}" onclick={() => (imp.entity_type = id as number)}>{label}</button>
					{/each}
				</div>
			</div>
			<div>
				<label class="label" for="im-text">Addresses</label>
				<textarea id="im-text" class="input h-40 font-mono text-xs" bind:value={imp.text} placeholder={'One per line: email, first name, last name, company\nor a CSV file with a header row'}></textarea>
				<div class="mt-1 flex items-center gap-3 text-xs text-slate-500">
					<label class="cursor-pointer text-brand-700 hover:underline">Choose a CSV file<input type="file" accept=".csv,.txt,text/csv,text/plain" class="sr-only" onchange={readFile} /></label>
					<span>{impRows.length} row{impRows.length === 1 ? '' : 's'}</span>
				</div>
			</div>
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={imp.create_missing} /> Add addresses that aren't in the CRM yet as new {imp.entity_type === 1 ? 'leads' : 'contacts'}</label>
		</form>
	{/if}
	{#snippet footer()}
		{#if impResult}
			<button class="btn btn-primary" onclick={() => (imp.open = false)}>Done</button>
		{:else}
			<button class="btn btn-secondary" onclick={() => (imp.open = false)}>Cancel</button>
			<button class="btn btn-primary" type="submit" form="imp-form" disabled={imp.busy || !impRows.length}>{imp.busy ? 'Importing…' : 'Import'}</button>
		{/if}
	{/snippet}
</Modal>
