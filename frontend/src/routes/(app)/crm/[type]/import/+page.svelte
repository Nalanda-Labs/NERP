<script lang="ts">
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { crm, type CrmType, linksCompany } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	const code = $derived(page.params.type!);
	let type = $state<CrmType | null>(null);
	let file = $state<File | null>(null);
	let delimiter = $state(',');
	let header = $state<string[]>([]);
	let sample = $state<string[][]>([]);
	let hasHeader = $state(true);
	let mapping = $state<Record<number, string>>({});
	let duplicates = $state<'skip' | 'update' | 'create'>('skip');
	let job = $state<string | null>(null);
	let status = $state<{ state: string; processed?: number; created?: number; updated?: number; skipped?: number; errors?: { line: number; error: string }[] } | null>(null);

	onMount(async () => {
		await crm.load();
		type = crm.type(code) ?? null;
	});

	// Field choices for this record type.
	const targets = $derived.by(() => {
		if (!type) return [];
		const out: { key: string; label: string }[] = [];
		for (const f of type.system_fields) {
			if (['address', 'assigned_to'].includes(f.code)) continue;
			out.push({ key: f.code, label: f.label });
		}
		if (type.system_fields.some((f) => f.code === 'address'))
			for (const [k, l] of [['line1', 'Street'], ['line2', 'Address line 2'], ['city', 'City'], ['region', 'Region'], ['postal_code', 'Postal code'], ['country', 'Country (ISO code)']])
				out.push({ key: `address.${k}`, label: `Address: ${l}` });
		out.push({ key: 'assigned_email', label: 'Responsible (email)' });
		if (type.has_stages) out.push({ key: 'stage_name', label: 'Stage (name)' });
		if (linksCompany(type)) out.push({ key: 'company', label: 'Company (name, created if new)' });
		for (const d of type.custom_fields) out.push({ key: d.code, label: d.label });
		return out;
	});

	// Minimal RFC 4180 parser for the preview (the server does the real parsing).
	function parse(text: string, d: string): string[][] {
		const rows: string[][] = [];
		let row: string[] = [], cell = '', quoted = false;
		for (let i = 0; i < text.length; i++) {
			const c = text[i]!;
			if (quoted) {
				if (c === '"' && text[i + 1] === '"') (cell += '"'), i++;
				else if (c === '"') quoted = false;
				else cell += c;
			} else if (c === '"') quoted = true;
			else if (c === d) row.push(cell), (cell = '');
			else if (c === '\n' || c === '\r') {
				if (c === '\r' && text[i + 1] === '\n') i++;
				row.push(cell), rows.push(row), (row = []), (cell = '');
				if (rows.length > 6) break;
			} else cell += c;
		}
		if (cell || row.length) row.push(cell), rows.push(row);
		return rows;
	}

	async function pick(f: File | undefined) {
		if (!f) return;
		// The column guess needs the record type's fields; a file can be chosen before
		// they have loaded.
		if (!type) {
			await crm.load();
			type = crm.type(code) ?? null;
		}
		file = f;
		const head = await f.slice(0, 64 * 1024).text();
		delimiter = (head.split('\n')[0]!.match(/;/g)?.length ?? 0) > (head.split('\n')[0]!.match(/,/g)?.length ?? 0) ? ';' : ',';
		const rows = parse(head, delimiter);
		header = rows[0] ?? [];
		sample = rows.slice(1, 6);
		// Guess the mapping from header names.
		const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, '');
		const guess: Record<number, string> = {};
		header.forEach((h, i) => {
			const n = norm(h);
			const hit = targets.find((t) => norm(t.label) === n || norm(t.key) === n) ??
				(n.includes('phone') || n.includes('mobile') ? targets.find((t) => t.key === 'phone') : undefined) ??
				(n.includes('mail') ? targets.find((t) => t.key === 'email') : undefined) ??
				(n === 'first' || n === 'firstname' ? targets.find((t) => t.key === 'first_name') : undefined) ??
				(n === 'last' || n === 'surname' || n === 'lastname' ? targets.find((t) => t.key === 'last_name') : undefined) ??
				(n === 'company' || n === 'organization' ? targets.find((t) => t.key === 'company' || t.key === 'company_name') : undefined) ??
				(n === 'country' ? targets.find((t) => t.key === 'address.country') : undefined) ??
				(n === 'city' ? targets.find((t) => t.key === 'address.city') : undefined);
			if (hit) guess[i] = hit.key;
		});
		mapping = guess;
	}

	async function start() {
		try {
			const fd = new FormData();
			fd.append('file', file!);
			const meta = await api.post<{ id: string }>('/files', fd);
			const map = Object.fromEntries(Object.entries(mapping).filter(([, v]) => v));
			const r = await api.post<{ job_id: string }>(`/crm/${code}/import`, { file_id: meta.id, mapping: map, has_header: hasHeader, duplicates, delimiter });
			job = r.job_id;
			poll();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function poll() {
		if (!job) return;
		status = await api.get(`/crm/imports/${job}`);
		if (status?.state !== 'done') setTimeout(poll, 1000);
	}
</script>

<svelte:head><title>Import {type?.name_plural ?? ''} · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<a href="/crm/{code}" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> {type?.name_plural}</a>
	<h1 class="text-2xl font-semibold text-slate-900">Import {type?.name_plural.toLowerCase()}</h1>

	{#if status}
		<section class="card space-y-3 p-6">
			<h2 class="text-lg text-slate-900">{status.state === 'done' ? 'Import finished' : 'Importing…'}</h2>
			<p class="text-sm text-slate-600">{status.processed ?? 0} rows processed · {status.created ?? 0} created · {status.updated ?? 0} updated · {status.skipped ?? 0} skipped as duplicates · {status.errors?.length ?? 0} failed</p>
			{#if status.errors?.length}
				<ul class="max-h-60 overflow-auto rounded-xl bg-red-50 p-3 text-xs text-red-800">
					{#each status.errors as e (e.line)}<li>Line {e.line}: {e.error}</li>{/each}
				</ul>
			{/if}
			{#if status.state === 'done'}<a class="btn btn-primary" href="/crm/{code}">View {type?.name_plural.toLowerCase()}</a>{/if}
		</section>
	{:else if !file}
		<label class="card flex cursor-pointer flex-col items-center gap-2 border-2 border-dashed border-slate-200 p-14 text-center hover:border-brand-400">
			<Icon name="upload" class="size-8 text-slate-400" />
			<span class="font-medium text-slate-800">Choose a CSV file</span>
			<span class="text-sm text-slate-500">UTF-8, comma or semicolon separated. Phones/emails can hold several values separated by ";".</span>
			<input type="file" accept=".csv,text/csv" class="hidden" onchange={(e) => pick(e.currentTarget.files?.[0])} />
		</label>
	{:else}
		<section class="card overflow-x-auto p-4">
			<div class="mb-3 flex flex-wrap items-center gap-4 text-sm">
				<span class="font-medium text-slate-800">{file.name}</span>
				<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={hasHeader} /> First row is a header</label>
				<label class="flex items-center gap-2">Separator <select class="input w-24" bind:value={delimiter} onchange={() => pick(file!)}><option value=",">,</option><option value=";">;</option><option value={'\t'}>Tab</option></select></label>
				<button class="btn btn-ghost ml-auto" onclick={() => ((file = null), (header = []))}>Choose another file</button>
			</div>
			<table class="min-w-full text-sm">
				<thead>
					<tr>
						{#each header as h, i (i)}
							<th class="min-w-44 px-2 pb-2 text-left align-bottom">
								<div class="mb-1 truncate text-xs font-medium text-slate-500">{hasHeader ? h : `Column ${i + 1}`}</div>
								<select class="input" bind:value={mapping[i]} aria-label="Field for column {i + 1}">
									<option value="">— Skip —</option>
									{#each targets as t (t.key)}<option value={t.key}>{t.label}</option>{/each}
								</select>
							</th>
						{/each}
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-50 text-slate-600">
					{#each hasHeader ? sample : [header, ...sample] as row, r (r)}
						<tr>{#each header as _, i (i)}<td class="truncate px-2 py-1.5">{row[i] ?? ''}</td>{/each}</tr>
					{/each}
				</tbody>
			</table>
		</section>
		<section class="card flex flex-wrap items-center gap-4 p-4 text-sm">
			<span class="font-medium text-slate-700">If a row matches an existing record by phone or email:</span>
			{#each [['skip', 'Skip it'], ['update', 'Update the existing record'], ['create', 'Create a new one anyway']] as [v, l] (v)}
				<label class="flex items-center gap-1.5"><input type="radio" class="text-brand-600" value={v} bind:group={duplicates} /> {l}</label>
			{/each}
			<button class="btn btn-primary ml-auto" disabled={!Object.values(mapping).some(Boolean)} onclick={start}>Start import</button>
		</section>
	{/if}
</div>
