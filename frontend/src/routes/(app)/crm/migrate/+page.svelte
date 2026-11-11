<script lang="ts">
	// Moving over from another CRM: upload the export (CSV files or ZIP archives), check what
	// was recognised — each file's kind and columns, pipelines and stages, the people records
	// belong to — run it in the background, and see (or undo) what it did.
	import { onDestroy, onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { crm } from '#lib/crm.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type FileInfo = {
		name: string;
		kind: string;
		rows: number;
		columns: string[];
		mapping: Record<string, string>;
		samples: string[][];
	};
	type Analysis = {
		source: string;
		files: FileInfo[];
		fields: Record<string, string[]>;
		pipelines: { value: string; count: number; category_id: string | null }[];
		stages: {
			kind: string;
			pipeline: string;
			value: string;
			count: number;
			semantics: string;
			stage_id: string | null;
		}[];
		owners: { value: string; count: number; via: string | null; user_id: string | null }[];
		currencies: { code: string; count: number; enabled: boolean }[];
	};
	type Counts = Record<string, { created: number; updated: number; skipped: number; failed: number }>;
	type Migration = {
		id: string;
		source: string;
		status: 'queued' | 'running' | 'done' | 'failed' | 'undone';
		progress: {
			done?: number;
			total?: number;
			counts?: Counts;
			errors?: { file: string; line: number; error: string }[];
			error?: string;
		};
		created_at: string;
		finished_at: string | null;
		undone_at: string | null;
		files: string[] | null;
		created_by_name: string | null;
	};
	type Category = {
		id: string;
		name: string;
		stages: { id: string; name: string; semantics: string }[];
	};

	const SOURCES: Record<string, { name: string; how: string }> = {
		hubspot: {
			name: 'HubSpot',
			how: 'In HubSpot, open each of Contacts, Companies and Deals, choose Export (CSV, all properties). Upload the files (or a ZIP of them) together.'
		},
		salesforce: {
			name: 'Salesforce',
			how: 'Setup → Data Export → Export Now (include all data). Upload the ZIP it emails you: Account, Contact, Lead, Opportunity, Task, Note and User are used.'
		},
		pipedrive: {
			name: 'Pipedrive',
			how: 'Settings → Export data: export Organizations, People, Deals and Activities as CSV, and upload them together.'
		},
		zoho: {
			name: 'Zoho CRM',
			how: 'Setup → Data Administration → Export: export Accounts, Contacts, Leads, Deals, Tasks and Notes (CSV), and upload them together.'
		},
		other: {
			name: 'Another system',
			how: 'Export companies, contacts, leads and deals as CSV files (one per kind, with a header row) and upload them together; you match the columns below.'
		}
	};
	const KINDS: Record<string, string> = {
		company: 'Companies',
		contact: 'Contacts',
		lead: 'Leads',
		deal: 'Deals',
		activity: 'Activities and tasks',
		note: 'Notes',
		user: 'Users (to match owners)',
		skip: 'Leave out'
	};
	const FIELD: Record<string, string> = {
		id: 'Its id (links)',
		name: 'Name',
		title: 'Title',
		first_name: 'First name',
		last_name: 'Last name',
		full_name: 'Full name',
		job_title: 'Job title',
		email: 'Email',
		phone: 'Phone',
		website: 'Website',
		industry: 'Industry (to the comments)',
		employees: 'Employees (to the comments)',
		annual_revenue: 'Annual revenue',
		owner: 'Owner / responsible',
		description: 'Description / comments',
		created_at: 'Created',
		company_ref: 'Company (its id)',
		company_name: 'Company (name)',
		contact_ref: 'Contact (its id)',
		contact_name: 'Contact (name)',
		birthdate: 'Date of birth',
		source: 'Source',
		status: 'Status (stage)',
		amount: 'Amount',
		currency: 'Currency',
		pipeline: 'Pipeline',
		stage: 'Stage',
		close_date: 'Close date',
		probability: 'Probability',
		type: 'Type',
		subject: 'Subject',
		due: 'Due',
		done: 'Done / status',
		parent_ref: 'Belongs to (its id)',
		body: 'Text',
		'address.line1': 'Address: street',
		'address.city': 'Address: city',
		'address.region': 'Address: region',
		'address.postal_code': 'Address: postal code',
		'address.country': 'Address: country (ISO code)'
	};

	let source = $state('hubspot');
	let fileIds = $state<string[]>([]);
	let uploading = $state(false);
	let a = $state<Analysis | null>(null);
	let pipelines = $state<Record<string, string>>({});
	let stages = $state<Record<string, string>>({});
	let owners = $state<Record<string, string>>({});
	let duplicates = $state<'update' | 'skip'>('update');
	let dealCats = $state<Category[]>([]);
	let leadStages = $state<Category['stages']>([]);
	let users = $state<{ id: string; name: string; email: string }[]>([]);
	let history = $state<Migration[]>([]);
	let current = $state<Migration | null>(null);
	let timer: ReturnType<typeof setInterval> | undefined;

	async function loadHistory() {
		try {
			history = (await api.get<{ items: Migration[] }>('/crm/migrations')).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(async () => {
		await loadHistory();
		try {
			await crm.load(true);
			const people = await api.get<{ items: { id: string; first_name: string; last_name: string; email: string }[] }>(
				'/users?limit=200'
			);
			dealCats = crm.type('deal')?.categories ?? [];
			leadStages = crm.type('lead')?.categories[0]?.stages ?? [];
			users = people.items.map((u) => ({ id: u.id, name: `${u.first_name} ${u.last_name}`, email: u.email }));
		} catch (e) {
			toasts.error(e);
		}
		const running = history.find((m) => m.status === 'queued' || m.status === 'running');
		if (running) watch(running.id);
	});
	onDestroy(() => clearInterval(timer));

	async function upload(e: Event) {
		const input = e.target as HTMLInputElement;
		const files = [...(input.files ?? [])];
		if (!files.length) return;
		uploading = true;
		try {
			for (const f of files) {
				const fd = new FormData();
				fd.append('file', f);
				fileIds.push((await api.post<{ id: string }>('/files', fd)).id);
			}
			await analyze();
		} catch (err) {
			toasts.error(err);
		} finally {
			uploading = false;
			input.value = '';
		}
	}
	async function analyze() {
		a = await api.post<Analysis>('/crm/migrations/analyze', { file_ids: fileIds, source });
		source = a.source;
		pipelines = Object.fromEntries(a.pipelines.map((p) => [p.value, p.category_id ?? 'new']));
		stages = Object.fromEntries(
			a.stages.map((s) => [`${s.kind}|${s.pipeline}|${s.value}`, s.stage_id ?? (s.kind === 'deal' ? 'new' : '')])
		);
		owners = Object.fromEntries(a.owners.map((o) => [o.value, o.user_id ?? '']));
	}
	function reset() {
		a = null;
		fileIds = [];
	}
	// The stages to choose from for a deal stage: those of the pipeline it goes to.
	const stagesFor = (s: Analysis['stages'][number]) =>
		s.kind === 'lead' ? leadStages : (dealCats.find((c) => c.id === pipelines[s.pipeline])?.stages ?? []);

	async function run() {
		if (!a) return;
		try {
			const m = await api.post<Migration>('/crm/migrations', {
				file_ids: fileIds,
				source,
				files: a.files.map((f) => ({ name: f.name, kind: f.kind, mapping: f.mapping })),
				pipelines,
				stages: Object.fromEntries(Object.entries(stages).filter(([, v]) => v)),
				owners: Object.fromEntries(Object.entries(owners).filter(([, v]) => v)),
				duplicates
			});
			a = null;
			fileIds = [];
			watch(m.id);
		} catch (e) {
			toasts.error(e);
		}
	}
	function watch(id: string) {
		clearInterval(timer);
		const tick = async () => {
			current = await api.get<Migration>(`/crm/migrations/${id}`);
			if (current.status !== 'queued' && current.status !== 'running') {
				clearInterval(timer);
				await loadHistory();
			}
		};
		tick();
		timer = setInterval(tick, 1500);
	}
	async function undo(m: Migration) {
		if (!confirm('Move everything this migration created to the recycle bin (and remove its activities and notes)?')) return;
		try {
			const r = await api.post<{ records: number; activities: number; notes: number }>(`/crm/migrations/${m.id}/undo`);
			toasts.success(`Undone: ${r.records} records, ${r.activities} activities, ${r.notes} notes`);
			if (current?.id === m.id) current = await api.get<Migration>(`/crm/migrations/${m.id}`);
			await loadHistory();
		} catch (e) {
			toasts.error(e);
		}
	}
	const sum = (c: Counts | undefined, k: 'created' | 'updated' | 'skipped' | 'failed') =>
		Object.values(c ?? {}).reduce((n, x) => n + x[k], 0);
</script>

<svelte:head><title>Move from another CRM · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">Move from another CRM</h1>
		<p class="text-sm text-slate-500">
			Companies, contacts, leads and deals, with their activities and notes and the links between
			them. Nothing is sent and no automation runs while records come in; running the same export
			again updates what came over before instead of duplicating it.
		</p>
	</div>

	{#if current}
		<section class="card space-y-2 p-4 text-sm" aria-label="Migration" data-migration={current.status}>
			<div class="flex items-center gap-2">
				<h2 class="font-semibold text-slate-900">
					{SOURCES[current.source]?.name ?? current.source}:
					{current.status === 'queued' || current.status === 'running'
						? 'bringing records over…'
						: current.status === 'done'
							? 'done'
							: current.status === 'failed'
								? 'stopped'
								: 'undone'}
				</h2>
				{#if current.progress.total}<span class="text-slate-500 tabular-nums" data-progress
						>{current.progress.done ?? 0} of {current.progress.total} rows</span
					>{/if}
				{#if current.status === 'done'}<button class="btn btn-ghost ml-auto text-red-700" onclick={() => undo(current!)}
						>Undo</button
					>{/if}
			</div>
			{#if current.progress.total}
				<div class="h-1.5 rounded bg-slate-100">
					<div
						class="h-1.5 rounded bg-brand-500"
						style="width: {Math.round(((current.progress.done ?? 0) / current.progress.total) * 100)}%"
					></div>
				</div>
			{/if}
			{#if current.progress.error}<p class="text-red-700">{current.progress.error}</p>{/if}
			{#if current.progress.counts}
				<table class="w-full max-w-lg">
					<thead><tr class="text-left text-xs text-slate-500"><th></th><th>Created</th><th>Updated</th><th>Skipped</th><th>Failed</th></tr></thead>
					<tbody>
						{#each Object.entries(current.progress.counts) as [k, c] (k)}
							<tr data-count={k}><td class="py-0.5">{KINDS[k] ?? k}</td><td>{c.created}</td><td>{c.updated}</td><td>{c.skipped}</td><td class={c.failed ? 'text-red-700' : ''}>{c.failed}</td></tr>
						{/each}
					</tbody>
				</table>
			{/if}
			{#if current.progress.errors?.length}
				<details>
					<summary class="cursor-pointer text-red-700">{current.progress.errors.length} rows could not be brought over</summary>
					<ul class="mt-1 max-h-64 overflow-auto text-xs">
						{#each current.progress.errors as e, i (i)}<li>{e.file}, line {e.line}: {e.error}</li>{/each}
					</ul>
				</details>
			{/if}
		</section>
	{/if}

	{#if !a}
		<section class="card space-y-3 p-4 text-sm" aria-label="Upload the export">
			<h2 class="font-semibold text-slate-900">1. The export</h2>
			<div class="flex flex-wrap gap-2" role="radiogroup" aria-label="From">
				{#each Object.entries(SOURCES) as [k, s] (k)}
					<label class="btn {source === k ? 'btn-primary' : 'btn-secondary'}">
						<input type="radio" class="sr-only" name="source" value={k} bind:group={source} />{s.name}
					</label>
				{/each}
			</div>
			<p class="text-slate-600">{SOURCES[source].how}</p>
			<label class="btn btn-secondary">
				{uploading ? 'Reading…' : 'Choose CSV or ZIP files'}
				<input type="file" class="sr-only" multiple accept=".csv,.zip,text/csv,application/zip" onchange={upload} disabled={uploading} data-migrate-files />
			</label>
		</section>
	{:else}
		<section class="card space-y-3 p-4 text-sm" aria-label="Files">
			<div class="flex items-center gap-2">
				<h2 class="font-semibold text-slate-900">2. What is in it ({SOURCES[a.source]?.name ?? a.source})</h2>
				<button class="btn btn-ghost ml-auto" onclick={reset}>Start over</button>
			</div>
			{#each a.files as f (f.name)}
				<details class="rounded-lg border border-slate-100 p-3" data-file={f.name} open={a.files.length < 3}>
					<summary class="flex cursor-pointer items-center gap-2">
						<span class="font-medium">{f.name}</span>
						<span class="text-slate-500">{f.rows} rows</span>
						<select class="input ml-auto w-56" bind:value={f.kind} aria-label="{f.name}: what it holds" onclick={(e) => e.stopPropagation()} data-kind>
							{#each Object.entries(KINDS) as [k, l] (k)}<option value={k}>{l}</option>{/each}
						</select>
					</summary>
					{#if f.kind !== 'skip'}
						<table class="mt-2 w-full text-xs">
							<thead><tr class="text-left text-slate-500"><th class="py-1">Column</th><th>Example</th><th>Goes to</th></tr></thead>
							<tbody class="divide-y divide-slate-50">
								{#each f.columns as c, i (c)}
									<tr data-column={c}>
										<td class="py-1 pr-2">{c}</td>
										<td class="max-w-48 truncate pr-2 text-slate-500">{f.samples[0]?.[i] ?? ''}</td>
										<td>
											<select class="input py-1 text-xs" value={f.mapping[c] ?? ''} onchange={(e) => {
												const v = (e.target as HTMLSelectElement).value;
												if (v) f.mapping[c] = v;
												else delete f.mapping[c];
											}} aria-label="{c} goes to">
												<option value="">—</option>
												{#each a.fields[f.kind] ?? [] as fld (fld)}<option value={fld}>{FIELD[fld] ?? fld}</option>{/each}
											</select>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					{/if}
				</details>
			{/each}
		</section>

		{#if a.pipelines.length || a.stages.length}
			<section class="card space-y-3 p-4 text-sm" aria-label="Pipelines and stages">
				<h2 class="font-semibold text-slate-900">Pipelines and stages</h2>
				{#each a.pipelines as p (p.value)}
					<div class="flex items-center gap-2" data-pipeline={p.value}>
						<span class="w-56">Deals in <strong>{p.value || '(no pipeline)'}</strong> ({p.count})</span>
						<select class="input w-64" bind:value={pipelines[p.value]} aria-label="Pipeline for {p.value || 'deals without one'}">
							<option value="new">A new pipeline{p.value ? ` “${p.value}”` : ''}</option>
							{#each dealCats as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
						</select>
					</div>
				{/each}
				<table class="w-full text-xs">
					<thead><tr class="text-left text-slate-500"><th class="py-1">From</th><th>Records</th><th>Goes to</th></tr></thead>
					<tbody class="divide-y divide-slate-50">
						{#each a.stages as s (`${s.kind}|${s.pipeline}|${s.value}`)}
							{@const key = `${s.kind}|${s.pipeline}|${s.value}`}
							<tr data-stage={s.value}>
								<td class="py-1">{s.kind === 'lead' ? 'Lead status' : s.pipeline || 'Deal stage'} › <strong>{s.value}</strong></td>
								<td>{s.count}</td>
								<td>
									<select class="input py-1 text-xs" bind:value={stages[key]} aria-label="Stage for {s.value}">
										{#if s.kind === 'deal'}<option value="new">The same name (made if needed)</option>{:else}<option value="">The first stage</option>{/if}
										{#each stagesFor(s) as st (st.id)}<option value={st.id}>{st.name}</option>{/each}
									</select>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</section>
		{/if}

		{#if a.owners.length}
			<section class="card space-y-2 p-4 text-sm" aria-label="Owners">
				<h2 class="font-semibold text-slate-900">Who records belong to</h2>
				<p class="text-xs text-slate-500">Unmatched owners' records go to you.</p>
				{#each a.owners as o (o.value)}
					<div class="flex items-center gap-2" data-owner={o.value}>
						<span class="w-64 truncate">{o.value}{#if o.via}<span class="text-slate-500"> ({o.via})</span>{/if} · {o.count}</span>
						<select class="input w-64" bind:value={owners[o.value]} aria-label="Employee for {o.value}">
							<option value="">Me</option>
							{#each users as u (u.id)}<option value={u.id}>{u.name} · {u.email}</option>{/each}
						</select>
					</div>
				{/each}
			</section>
		{/if}

		{#if a.currencies.some((c) => !c.enabled)}
			<p class="rounded-lg bg-amber-50 p-3 text-sm text-amber-900" data-currencies>
				Not enabled in CRM settings: {a.currencies.filter((c) => !c.enabled).map((c) => c.code).join(', ')}. Those deals come
				over in the base currency unless you enable them first.
			</p>
		{/if}

		<section class="card flex flex-wrap items-center gap-3 p-4 text-sm" aria-label="Run">
			<label class="flex items-center gap-2">Records brought over before, or already here:
				<select class="input w-48" bind:value={duplicates}>
					<option value="update">Update them</option>
					<option value="skip">Leave them as they are</option>
				</select>
			</label>
			<button class="btn btn-primary ml-auto" onclick={run} data-migrate-run>3. Bring them over</button>
		</section>
	{/if}

	{#if history.length}
		<section class="card text-sm" aria-label="Earlier migrations">
			<table class="w-full">
				<thead><tr class="border-b border-slate-100 text-left text-xs text-slate-500"><th class="px-4 py-2">When</th><th>From</th><th>Result</th><th>By</th><th></th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each history as m (m.id)}
						<tr data-history={m.id}>
							<td class="px-4 py-2">{formatDateTime(m.created_at)}</td>
							<td>{SOURCES[m.source]?.name ?? m.source}</td>
							<td>
								{m.status === 'done'
									? `${sum(m.progress.counts, 'created')} created, ${sum(m.progress.counts, 'updated')} updated${sum(m.progress.counts, 'failed') ? `, ${sum(m.progress.counts, 'failed')} failed` : ''}`
									: m.status}
							</td>
							<td>{m.created_by_name ?? ''}</td>
							<td class="pr-4 text-right">
								<button class="text-brand-600 hover:underline" onclick={() => watch(m.id)}>Details</button>
								{#if m.status === 'done'}<button class="ml-3 text-red-600 hover:underline" onclick={() => undo(m)}>Undo</button>{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>
