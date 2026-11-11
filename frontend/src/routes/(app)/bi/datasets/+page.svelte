<script lang="ts">
	// BI datasets: NERP's own data, CSV/JSON files (uploaded, or from a URL and refreshed),
	// and tables or queries in outside PostgreSQL databases. Each can be looked at,
	// downloaded as CSV, refreshed or removed.
	import { api, downloadUrl } from '#lib/api.ts';
	import { show, type Dataset } from '#lib/bi.ts';
	import { formatRelative } from '#lib/format.ts';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { FileMeta } from '#lib/types.ts';

	type Source = {
		id: string;
		name: string;
		kind: 'postgres';
		host: string;
		port: number;
		database: string;
		username: string;
		has_password: boolean;
		tls: boolean;
	};
	type Detail = Dataset & { preview: unknown[][] | { error: string }; used_by?: string[] };
	let items = $state<Dataset[]>([]);
	let sources = $state<Source[]>([]);
	let current = $state<Detail | null>(null);
	let busy = $state(false);

	async function load() {
		try {
			[items, sources] = await Promise.all([
				api.get<{ items: Dataset[] }>('/bi/datasets').then((r) => r.items),
				api.get<{ items: Source[] }>('/bi/sources').then((r) => r.items)
			]);
		} catch (e) {
			toasts.error(e);
		}
	}
	load();
	async function openDs(key: string) {
		current = null;
		try {
			current = await api.get<Detail>(`/bi/datasets/${key}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	const groups = $derived([...new Set(items.map((d) => d.group))]);
	const KIND: Record<string, string> = { nerp: 'NERP', csv: 'File', url: 'URL', sql: 'Database' };

	async function uploadFile(f: File) {
		const fd = new FormData();
		fd.append('file', f);
		return (await api.post<FileMeta>('/files', fd)).id;
	}

	// New datasets.
	let addOpen = $state<'' | 'csv' | 'url' | 'sql'>('');
	let form = $state({
		name: '',
		url: '',
		refresh_hours: 24 as number | null,
		source_id: '',
		table: '',
		query: ''
	});
	let file = $state<File | null>(null);
	let tables = $state<string[]>([]);
	let useQuery = $state(false);
	function startAdd(kind: 'csv' | 'url' | 'sql') {
		form = {
			name: '',
			url: '',
			refresh_hours: 24,
			source_id: sources[0]?.id ?? '',
			table: '',
			query: ''
		};
		file = null;
		tables = [];
		useQuery = false;
		addOpen = kind;
		if (kind === 'sql' && form.source_id) loadTables();
	}
	async function loadTables() {
		tables = [];
		if (!form.source_id) return;
		try {
			tables = (await api.get<{ items: string[] }>(`/bi/sources/${form.source_id}/tables`)).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function add(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const body: Record<string, unknown> = { name: form.name, kind: addOpen };
			if (addOpen === 'csv') body.file_id = await uploadFile(file!);
			if (addOpen === 'url')
				Object.assign(body, { url: form.url, refresh_hours: form.refresh_hours || null });
			if (addOpen === 'sql')
				Object.assign(body, {
					source_id: form.source_id,
					...(useQuery ? { query: form.query } : { table: form.table })
				});
			const d = await api.post<Detail>('/bi/datasets', body);
			addOpen = '';
			await load();
			current = d;
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}

	// Changing one.
	let replaceInput: HTMLInputElement | undefined = $state();
	async function refresh(fileId?: string) {
		busy = true;
		try {
			current = await api.post<Detail>(
				`/bi/datasets/${current!.key}/refresh`,
				fileId ? { file_id: fileId } : {}
			);
			await load();
			toasts.success('Refreshed');
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	async function change(body: object) {
		try {
			current = await api.patch<Detail>(`/bi/datasets/${current!.key}`, body);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function drop() {
		if (!confirm(`Delete the dataset “${current!.name}”?`)) return;
		try {
			await api.del(`/bi/datasets/${current!.key}`);
			current = null;
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	// Databases.
	let srcOpen = $state(false);
	let src = $state({
		id: '',
		name: '',
		kind: 'postgres',
		host: '',
		port: 5432,
		database: '',
		username: '',
		password: '',
		tls: false
	});
	function editSource(s?: Source) {
		src = s
			? { ...s, password: '' }
			: {
					id: '',
					name: '',
					kind: 'postgres',
					host: '',
					port: 5432,
					database: '',
					username: '',
					password: '',
					tls: false
				};
		srcOpen = true;
	}
	async function saveSource(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const { id, password, ...rest } = src;
			const body = { ...rest, ...(password ? { password } : {}) };
			if (id) await api.patch(`/bi/sources/${id}`, body);
			else await api.post('/bi/sources', { ...body, password });
			srcOpen = false;
			toasts.success('Connected');
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function dropSource(s: Source) {
		if (!confirm(`Remove the database “${s.name}” and its datasets?`)) return;
		try {
			await api.del(`/bi/sources/${s.id}`);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>BI datasets · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-2">
		<a class="btn btn-ghost px-2" href="/bi" aria-label="Dashboards">‹</a>
		<h1 class="mr-2 text-2xl font-semibold text-slate-900">Datasets</h1>
		<div class="ml-auto flex flex-wrap gap-2">
			<button class="btn btn-secondary" onclick={() => startAdd('csv')}>Upload a file</button>
			<button class="btn btn-secondary" onclick={() => startAdd('url')}>From a URL</button>
			<button
				class="btn btn-secondary"
				onclick={() => startAdd('sql')}
				disabled={!sources.length}
				title={sources.length ? '' : 'Add a database first'}>From a database</button
			>
		</div>
	</div>

	<div class="grid gap-4 lg:grid-cols-3">
		<div class="space-y-4">
			{#each groups as g (g)}
				<section class="card">
					<h2 class="border-b border-slate-100 px-4 py-2.5 text-sm font-semibold text-slate-900">
						{g}
					</h2>
					<ul class="divide-y divide-slate-50 text-sm">
						{#each items.filter((d) => d.group === g) as d (d.key)}
							<li>
								<button
									class="flex w-full items-center gap-2 px-4 py-2 text-left hover:bg-slate-50 {current?.key ===
									d.key
										? 'bg-brand-50'
										: ''}"
									onclick={() => openDs(d.key)}
									data-dataset={d.name}
								>
									<span class="flex-1">{d.name}</span>
									{#if d.last_error}<span class="text-xs text-red-600" title={d.last_error}>⚠</span
										>{/if}
									<span class="text-xs text-slate-400"
										>{d.kind === 'nerp' ? `${d.columns.length} columns` : KIND[d.kind]}</span
									>
								</button>
							</li>
						{/each}
					</ul>
				</section>
			{/each}
			<section class="card" aria-label="Databases">
				<div class="flex items-center border-b border-slate-100 px-4 py-2.5">
					<h2 class="flex-1 text-sm font-semibold text-slate-900">Outside databases</h2>
					<button class="text-sm text-brand-600 hover:underline" onclick={() => editSource()}
						>+ Add</button
					>
				</div>
				<ul class="divide-y divide-slate-50 text-sm">
					{#each sources as s (s.id)}
						<li class="flex items-center gap-2 px-4 py-2" data-source={s.name}>
							<span class="flex-1"
								><span class="font-medium">{s.name}</span><br /><span class="text-xs text-slate-500"
									>PostgreSQL · {s.username}@{s.host}:{s.port}/{s.database}</span
								></span
							>
							<button
								class="text-slate-500 hover:text-brand-700"
								aria-label="Change {s.name}"
								onclick={() => editSource(s)}>✎</button
							>
							<button
								class="text-slate-400 hover:text-red-600"
								aria-label="Remove {s.name}"
								onclick={() => dropSource(s)}>×</button
							>
						</li>
					{:else}<li class="px-4 py-2 text-slate-400">None yet.</li>{/each}
				</ul>
			</section>
		</div>

		<div class="lg:col-span-2">
			{#if current}
				<section class="card space-y-3 p-4 text-sm" aria-label={current.name}>
					<div class="flex flex-wrap items-center gap-2">
						{#if current.id}
							<input
								class="input w-72 py-1 font-semibold"
								aria-label="Dataset name"
								maxlength="200"
								value={current.name}
								onblur={(e) =>
									e.currentTarget.value !== current!.name &&
									change({ name: e.currentTarget.value })}
							/>
						{:else}<h2 class="text-lg font-semibold text-slate-900">{current.name}</h2>{/if}
						<div class="ml-auto flex gap-2">
							<button
								class="btn btn-secondary"
								onclick={() =>
									downloadUrl(
										`/api/bi/datasets/${current!.key}/export.csv`,
										`${current!.name}.csv`
									).catch(toasts.error)}>Download CSV</button
							>
							{#if current.kind === 'url' || current.kind === 'sql'}<button
									class="btn btn-secondary"
									disabled={busy}
									onclick={() => refresh()}
									>{current.kind === 'url' ? 'Fetch now' : 'Read columns again'}</button
								>{/if}
							{#if current.kind === 'csv'}
								<button
									class="btn btn-secondary"
									disabled={busy}
									onclick={() => replaceInput?.click()}>Replace the file</button
								>
								<input
									bind:this={replaceInput}
									type="file"
									accept=".csv,.json,text/csv,application/json"
									class="hidden"
									aria-label="New file"
									onchange={async (e) => {
										const f = e.currentTarget.files?.[0];
										if (f) refresh(await uploadFile(f));
									}}
								/>
							{/if}
							{#if current.id}<button class="btn btn-ghost hover:text-red-600" onclick={drop}
									>Delete</button
								>{/if}
						</div>
					</div>
					{#if current.id}
						<p class="text-slate-500">
							{current.row_count !== undefined && current.kind !== 'sql'
								? `${current.row_count} rows · `
								: ''}{current.source_name
								? `From ${current.source_name} · `
								: ''}{current.refreshed_at ? `read ${formatRelative(current.refreshed_at)}` : ''}
							{#if current.used_by?.length}
								· used on {current.used_by.join(', ')}{/if}
						</p>
						{#if current.last_error}<p class="rounded-lg bg-red-50 p-2 text-red-800">
								Last refresh failed: {current.last_error}
							</p>{/if}
						{#if current.kind === 'url'}
							<div class="flex flex-wrap items-end gap-2">
								<div class="flex-1">
									<label class="label" for="ds-url">URL</label><input
										id="ds-url"
										class="input"
										value={current.url}
										onchange={(e) => change({ url: e.currentTarget.value })}
									/>
								</div>
								<div>
									<label class="label" for="ds-every">Fetch again every (hours)</label><input
										id="ds-every"
										class="input w-40"
										type="number"
										min="1"
										max="744"
										placeholder="never"
										value={current.refresh_hours ?? ''}
										onchange={(e) =>
											change({
												refresh_hours: e.currentTarget.value ? Number(e.currentTarget.value) : null
											})}
									/>
								</div>
							</div>
						{/if}
						{#if current.kind === 'sql'}
							<div>
								<label class="label" for="ds-q">Query</label><textarea
									id="ds-q"
									class="input font-mono text-xs"
									rows="4"
									value={current.query}
									onchange={(e) => change({ query: e.currentTarget.value })}></textarea>
							</div>
						{/if}
					{/if}
					<div>
						<p class="label">Columns</p>
						<div class="flex flex-wrap gap-1">
							{#each current.columns as c (c.key)}<span
									class="badge bg-slate-100 text-slate-700"
									title={c.key}>{c.label} <span class="text-slate-400">· {c.type}</span></span
								>{/each}
						</div>
					</div>
					<div>
						<p class="label">First rows</p>
						{#if Array.isArray(current.preview)}
							<div class="max-h-96 overflow-auto rounded-lg border border-slate-100">
								<table class="w-full text-xs" data-preview>
									<thead class="sticky top-0 bg-slate-50"
										><tr
											>{#each current.columns.slice(0, 30) as c (c.key)}<th
													class="px-2 py-1 text-left font-medium whitespace-nowrap text-slate-600"
													>{c.label}</th
												>{/each}</tr
										></thead
									>
									<tbody>
										{#each current.preview as r, i (i)}<tr class="border-t border-slate-50"
												>{#each current.columns.slice(0, 30) as c, j (c.key)}<td
														class="px-2 py-1 whitespace-nowrap">{show(r[j], c.type)}</td
													>{/each}</tr
											>{:else}<tr><td class="px-2 py-2 text-slate-400">No rows.</td></tr>{/each}
									</tbody>
								</table>
							</div>
						{:else}<p class="text-red-700">{current.preview.error}</p>{/if}
					</div>
				</section>
			{:else}
				<p class="card p-6 text-sm text-slate-500">
					Pick a dataset to see its columns and first rows. Charts on dashboards are built from
					these. Every dataset can also be downloaded as CSV (with an API key too), for spreadsheets
					and other tools.
				</p>
			{/if}
		</div>
	</div>
</div>

<Modal
	open={!!addOpen}
	onclose={() => (addOpen = '')}
	title={addOpen === 'csv'
		? 'Upload a file'
		: addOpen === 'url'
			? 'A file from a URL'
			: 'From a database'}
>
	<form id="ds-add" class="space-y-3 text-sm" onsubmit={add}>
		<div>
			<label class="label" for="da-name">Name</label><input
				id="da-name"
				class="input"
				required
				maxlength="200"
				bind:value={form.name}
			/>
		</div>
		{#if addOpen === 'csv'}
			<div>
				<label class="label" for="da-file">CSV or JSON file (first line: column names)</label>
				<input
					id="da-file"
					class="input"
					type="file"
					accept=".csv,.json,text/csv,application/json"
					required
					onchange={(e) => (file = e.currentTarget.files?.[0] ?? null)}
				/>
				<p class="mt-1 text-xs text-slate-500">
					Up to 200,000 rows and 100 columns. Numbers, dates and yes/no columns are recognised.
				</p>
			</div>
		{:else if addOpen === 'url'}
			<div>
				<label class="label" for="da-url">URL of a CSV file or a JSON list</label><input
					id="da-url"
					class="input"
					type="url"
					required
					placeholder="https://…"
					bind:value={form.url}
				/>
			</div>
			<div>
				<label class="label" for="da-every">Fetch again every (hours)</label><input
					id="da-every"
					class="input w-40"
					type="number"
					min="1"
					max="744"
					placeholder="never"
					bind:value={form.refresh_hours}
				/>
			</div>
		{:else if addOpen === 'sql'}
			<div>
				<label class="label" for="da-src">Database</label>
				<select id="da-src" class="input" bind:value={form.source_id} onchange={loadTables}
					>{#each sources as s (s.id)}<option value={s.id}>{s.name}</option>{/each}</select
				>
			</div>
			<label class="flex items-center gap-2"
				><input type="checkbox" class="rounded border-slate-300" bind:checked={useQuery} /> Write a query
				instead of picking a table</label
			>
			{#if useQuery}
				<div>
					<label class="label" for="da-q">Query (read only; one SELECT)</label><textarea
						id="da-q"
						class="input font-mono text-xs"
						rows="5"
						required
						bind:value={form.query}></textarea>
				</div>
			{:else}
				<div>
					<label class="label" for="da-t">Table</label><select
						id="da-t"
						class="input"
						required
						bind:value={form.table}
						><option value="">Pick a table…</option>{#each tables as t (t)}<option value={t}
								>{t}</option
							>{/each}</select
					>
				</div>
			{/if}
		{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (addOpen = '')}>Cancel</button>
		<button class="btn btn-primary" form="ds-add" disabled={busy}
			>{busy ? 'Reading…' : 'Add'}</button
		>
	{/snippet}
</Modal>

<Modal bind:open={srcOpen} title={src.id ? 'Change the PostgreSQL database' : 'Add a PostgreSQL database'}>
	<form id="ds-src" class="grid grid-cols-2 gap-3 text-sm" onsubmit={saveSource}>
		<div class="col-span-2">
			<label class="label" for="s-name">Name</label><input
				id="s-name"
				class="input"
				required
				maxlength="200"
				bind:value={src.name}
			/>
		</div>
		<div>
			<label class="label" for="s-host">Host</label><input
				id="s-host"
				class="input"
				required
				bind:value={src.host}
			/>
		</div>
		<div>
			<label class="label" for="s-port">Port</label><input
				id="s-port"
				class="input"
				type="number"
				min="1"
				max="65535"
				required
				bind:value={src.port}
			/>
		</div>
		<div>
			<label class="label" for="s-db">Database</label><input
				id="s-db"
				class="input"
				required
				bind:value={src.database}
			/>
		</div>
		<div>
			<label class="label" for="s-user">User</label><input
				id="s-user"
				class="input"
				required
				autocomplete="off"
				bind:value={src.username}
			/>
		</div>
		<div class="col-span-2">
			<label class="label" for="s-pw">Password</label><input
				id="s-pw"
				class="input"
				type="password"
				autocomplete="new-password"
				placeholder={src.id ? 'Unchanged' : ''}
				bind:value={src.password}
			/>
		</div>
		<label class="col-span-2 flex items-center gap-2"
			><input type="checkbox" class="rounded border-slate-300" bind:checked={src.tls} /> Require an encrypted
			connection</label
		>
		<p class="col-span-2 text-xs text-slate-500">
			NERP connects read-only, and stops any query after 20 seconds. A user that may only read is
			best.
		</p>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (srcOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="ds-src" disabled={busy}
			>{busy ? 'Connecting…' : 'Connect and save'}</button
		>
	{/snippet}
</Modal>
