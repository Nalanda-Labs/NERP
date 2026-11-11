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
	// A .vcf file: contacts as phones and address books export them (no columns to map).
	let vcard = $state<{ count: number; names: string[] } | null>(null);
	// Which mail program's export the columns look like.
	let source = $state<string | null>(null);
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

	// Column names of contact exports from mail programs → fields (checked before the guess).
	const PRESETS: { name: string; sign: string[]; map: Record<string, string> }[] = [
		{
			name: 'Google Contacts',
			sign: ['E-mail 1 - Value', 'Phone 1 - Value'],
			map: {
				'First Name': 'first_name', 'Given Name': 'first_name', 'Middle Name': 'middle_name', 'Additional Name': 'middle_name', 'Last Name': 'last_name', 'Family Name': 'last_name',
				'Name Prefix': 'honorific', 'Organization Name': 'company', 'Organization 1 - Name': 'company', 'Organization Title': 'job_title', 'Organization 1 - Title': 'job_title',
				Birthday: 'birthdate', Notes: 'comments', 'E-mail 1 - Value': 'email', 'E-mail 2 - Value': 'email', 'E-mail 3 - Value': 'email',
				'Phone 1 - Value': 'phone', 'Phone 2 - Value': 'phone', 'Phone 3 - Value': 'phone', 'Website 1 - Value': 'web',
				'Address 1 - Street': 'address.line1', 'Address 1 - City': 'address.city', 'Address 1 - Region': 'address.region', 'Address 1 - Postal Code': 'address.postal_code', 'Address 1 - Country': 'address.country'
			}
		},
		{
			name: 'Outlook',
			sign: ['E-mail Address', 'Mobile Phone'],
			map: {
				'First Name': 'first_name', 'Middle Name': 'middle_name', 'Last Name': 'last_name', Title: 'honorific', Company: 'company', 'Job Title': 'job_title',
				'Business Street': 'address.line1', 'Business City': 'address.city', 'Business State': 'address.region', 'Business Postal Code': 'address.postal_code', 'Business Country/Region': 'address.country',
				'Business Phone': 'phone', 'Business Phone 2': 'phone', 'Home Phone': 'phone', 'Mobile Phone': 'phone', 'Other Phone': 'phone',
				'E-mail Address': 'email', 'E-mail 2 Address': 'email', 'E-mail 3 Address': 'email', Birthday: 'birthdate', Notes: 'comments', 'Web Page': 'web'
			}
		},
		{
			name: 'Yahoo Mail',
			sign: ['First', 'Last', 'Email', 'Alternate Email 1'],
			map: {
				First: 'first_name', Middle: 'middle_name', Last: 'last_name', Email: 'email', 'Alternate Email 1': 'email', 'Alternate Email 2': 'email',
				Home: 'phone', Work: 'phone', Mobile: 'phone', Other: 'phone', Title: 'job_title', Company: 'company',
				'Work Address': 'address.line1', 'Work City': 'address.city', 'Work State': 'address.region', 'Work ZIP': 'address.postal_code', 'Work Country': 'address.country',
				'Business Website': 'web', 'Personal Website': 'web', Birthday: 'birthdate', Comments: 'comments'
			}
		}
	];

	/** A file's text: UTF-8, UTF-16 with a byte-order mark, or else Windows-1252 (older Outlook). */
	async function readText(f: Blob): Promise<string> {
		const b = new Uint8Array(await f.arrayBuffer());
		if (b[0] === 0xff && b[1] === 0xfe) return new TextDecoder('utf-16le').decode(b);
		if (b[0] === 0xfe && b[1] === 0xff) return new TextDecoder('utf-16be').decode(b);
		try {
			return new TextDecoder('utf-8', { fatal: true }).decode(b).replace(/^﻿/, '');
		} catch {
			return new TextDecoder('windows-1252').decode(b);
		}
	}

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
		source = null;
		vcard = null;
		const head = await readText(f.slice(0, 64 * 1024));
		if (/\.vcf$/i.test(f.name) || /^\s*BEGIN:VCARD/i.test(head)) {
			const all = await readText(f);
			const names = [...all.matchAll(/^FN[^:\r\n]*:(.*)$/gim)].map((m) => m[1]!.trim());
			vcard = { count: (all.match(/^BEGIN:VCARD/gim) ?? []).length, names: names.slice(0, 8) };
			return;
		}
		delimiter = (head.split('\n')[0]!.match(/;/g)?.length ?? 0) > (head.split('\n')[0]!.match(/,/g)?.length ?? 0) ? ';' : ',';
		const rows = parse(head, delimiter);
		header = rows[0] ?? [];
		sample = rows.slice(1, 6);
		// A known export's columns, or else a guess from the header names.
		const preset = PRESETS.find((p) => p.sign.every((x) => header.includes(x)));
		source = preset?.name ?? null;
		const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, '');
		const guess: Record<number, string> = {};
		header.forEach((h, i) => {
			const known = preset?.map[h.trim()];
			if (preset) {
				if (known && targets.some((t) => t.key === known || (known === 'company' && t.key === 'company_name'))) guess[i] = targets.some((t) => t.key === known) ? known : 'company_name';
				return;
			}
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
			const r = vcard
				? await api.post<{ job_id: string }>(`/crm/${code}/import`, { file_id: meta.id, mapping: {}, format: 'vcard', duplicates })
				: await api.post<{ job_id: string }>(`/crm/${code}/import`, { file_id: meta.id, mapping: map, has_header: hasHeader, duplicates, delimiter });
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
			<span class="font-medium text-slate-800">Choose a CSV{code === 'contact' ? ' or vCard' : ''} file</span>
			<span class="text-sm text-slate-500">Comma or semicolon separated; phones/emails can hold several values separated by ";".</span>
			{#if code === 'contact'}<span class="text-sm text-slate-500">Contacts exported from Gmail, Outlook or Yahoo Mail (CSV), or from phones and address books (.vcf), are recognised as they are.</span>{/if}
			<input type="file" accept={code === 'contact' ? '.csv,.vcf,text/csv,text/vcard' : '.csv,text/csv'} class="hidden" onchange={(e) => pick(e.currentTarget.files?.[0])} />
		</label>
	{:else if vcard}
		<section class="card space-y-3 p-5 text-sm" data-vcard-preview>
			<div class="flex items-center gap-3">
				<span class="font-medium text-slate-800">{file.name}</span>
				<span class="text-slate-500">{vcard.count} card{vcard.count === 1 ? '' : 's'}</span>
				<button class="btn btn-ghost ml-auto" onclick={() => ((file = null), (vcard = null))}>Choose another file</button>
			</div>
			{#if vcard.names.length}<p class="text-slate-600">{vcard.names.join(', ')}{vcard.count > vcard.names.length ? '…' : ''}</p>{/if}
			<p class="text-slate-500">Names, phones, emails, the company (found or added by name), job title, address, birthday and notes are taken from each card.</p>
		</section>
		<section class="card flex flex-wrap items-center gap-4 p-4 text-sm">
			<span class="font-medium text-slate-700">If a card matches an existing contact by phone or email:</span>
			{#each [['skip', 'Skip it'], ['update', 'Update the existing contact'], ['create', 'Create a new one anyway']] as [v, l] (v)}
				<label class="flex items-center gap-1.5"><input type="radio" class="text-brand-600" value={v} bind:group={duplicates} /> {l}</label>
			{/each}
			<button class="btn btn-primary ml-auto" disabled={!vcard.count} onclick={start}>Start import</button>
		</section>
	{:else}
		<section class="card overflow-x-auto p-4">
			<div class="mb-3 flex flex-wrap items-center gap-4 text-sm">
				<span class="font-medium text-slate-800">{file.name}</span>
				{#if source}<span class="badge bg-emerald-50 text-emerald-800" data-import-source>{source} export recognised</span>{/if}
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
