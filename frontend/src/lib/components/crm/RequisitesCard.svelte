<script lang="ts" module>
	import { api } from '#lib/api.ts';

	export type ReqField = { code: string; label: string; hint: string; person: boolean };
	export type ReqSchema = { country: string; fields: ReqField[]; bank: ReqField[] };
	const schemas = new Map<string, Promise<ReqSchema>>();
	/** Country field sets, fetched once per country. */
	export function reqSchema(country: string): Promise<ReqSchema> {
		if (!schemas.has(country)) schemas.set(country, api.get<ReqSchema>(`/crm/requisites/schema?country=${country}`));
		return schemas.get(country)!;
	}
</script>

<script lang="ts">
	// Legal details of a company or contact: registered name, the country's tax and
	// registration numbers, legal address and bank accounts.
	import { onMount } from 'svelte';
	import { allCountries, countryName, formatAddress, itemLink, type Address } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';
	import AddressEditor from './AddressEditor.svelte';

	type Bank = { id?: string; name: string; fields: Record<string, string>; is_default: boolean };
	type Requisite = {
		id: string;
		kind: 'company' | 'person';
		country: string;
		name: string;
		fields: Record<string, string>;
		address: Address | null;
		is_default: boolean;
		banks: Bank[];
	};
	type Duplicate = { item_id: string; entity_type: number; title: string; field: string; value: string };

	let { itemId, person = false, canWrite, defaultCountry = '', defaultName = '' }: { itemId: string; person?: boolean; canWrite: boolean; defaultCountry?: string; defaultName?: string } = $props();

	let items = $state<Requisite[]>([]);
	let loaded = $state<Record<string, ReqSchema>>({});
	const countries = allCountries();

	async function ensureSchema(c: string) {
		if (c && !loaded[c]) loaded[c] = await reqSchema(c);
	}
	async function load() {
		items = (await api.get<{ items: Requisite[] }>(`/crm/items/${itemId}/requisites`)).items;
		await Promise.all(items.map((r) => ensureSchema(r.country)));
	}
	onMount(load);

	/** Values in the country's field order (stored JSON objects come back reordered). */
	function ordered(country: string, values: Record<string, string>, bank = false): [string, string][] {
		const defs = (bank ? loaded[country]?.bank : loaded[country]?.fields) ?? [];
		const known = defs.filter((f) => values[f.code]).map((f) => [f.code, values[f.code]!] as [string, string]);
		const rest = Object.entries(values).filter(([k]) => !defs.some((f) => f.code === k));
		return [...known, ...rest];
	}
	const label = (country: string, code: string, bank = false) => (bank ? loaded[country]?.bank : loaded[country]?.fields)?.find((f) => f.code === code)?.label ?? code;

	// ----- editor -----
	let open = $state(false);
	let saving = $state(false);
	let draft = $state<{ id: string | null; kind: 'company' | 'person'; country: string; name: string; fields: Record<string, string>; address: Address | null; is_default: boolean; banks: Bank[] } | null>(null);
	let errors = $state<Record<string, string>>({});
	let dups = $state<Duplicate[]>([]);
	const schema = $derived(draft ? loaded[draft.country] : undefined);
	const visibleFields = $derived(schema?.fields.filter((f) => draft?.kind === 'company' || f.person) ?? []);

	async function edit(r: Requisite | null) {
		const country = r?.country ?? (defaultCountry || items[0]?.country || 'US');
		await ensureSchema(country);
		draft = r
			? { id: r.id, kind: r.kind, country: r.country, name: r.name, fields: { ...r.fields }, address: r.address, is_default: r.is_default, banks: r.banks.map((b) => ({ ...b, fields: { ...b.fields } })) }
			: { id: null, kind: person ? 'person' : 'company', country, name: defaultName, fields: {}, address: null, is_default: items.length === 0, banks: [] };
		errors = {};
		dups = [];
		open = true;
	}

	async function changeCountry(c: string) {
		await ensureSchema(c);
		if (!draft) return;
		// Keep only values whose field exists in the new country.
		const codes = new Set(loaded[c]!.fields.map((f) => f.code));
		const bankCodes = new Set(loaded[c]!.bank.map((f) => f.code));
		draft.fields = Object.fromEntries(Object.entries(draft.fields).filter(([k]) => codes.has(k)));
		draft.banks = draft.banks.map((b) => ({ ...b, fields: Object.fromEntries(Object.entries(b.fields).filter(([k]) => bankCodes.has(k))) }));
		draft.country = c;
		errors = {};
	}

	/** Checks a value as the user leaves the field: format, checksum, and other records with it. */
	async function check(code: string, value: string, key = code) {
		if (!draft) return;
		const r = await api.get<{ error: string | null; duplicates: Duplicate[] }>(
			`/crm/requisites/check?country=${draft.country}&code=${code}&value=${encodeURIComponent(value ?? '')}&item_id=${itemId}`
		);
		errors = { ...errors, [key]: r.error ?? '' };
		const others = dups.filter((d) => d.field !== label(draft!.country, code));
		dups = [...others, ...r.duplicates];
	}

	async function save() {
		if (!draft) return;
		saving = true;
		try {
			const body = { kind: draft.kind, country: draft.country, name: draft.name, fields: draft.fields, address: draft.address ?? undefined, is_default: draft.is_default, banks: draft.banks };
			const r = draft.id ? await api.patch<Requisite & { duplicates: Duplicate[] }>(`/crm/requisites/${draft.id}`, body) : await api.post<Requisite & { duplicates: Duplicate[] }>(`/crm/items/${itemId}/requisites`, body);
			open = false;
			await load();
			if (r.duplicates.length) toasts.success(`Saved. ${r.duplicates.length === 1 ? 'Another record has' : `${r.duplicates.length} other records have`} the same ${r.duplicates[0]!.field}: ${r.duplicates.map((d) => d.title).join(', ')}`);
			else toasts.success('Requisites saved');
		} catch (e) {
			toasts.error(e);
		} finally {
			saving = false;
		}
	}

	async function remove(r: Requisite) {
		if (!confirm(`Delete the requisites "${r.name}"?`)) return;
		try {
			await api.del(`/crm/requisites/${r.id}`);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	async function makeDefault(r: Requisite) {
		try {
			await api.patch(`/crm/requisites/${r.id}`, { kind: r.kind, country: r.country, name: r.name, fields: r.fields, address: r.address ?? undefined, is_default: true, banks: r.banks });
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	function addBank() {
		draft!.banks = [...draft!.banks, { name: '', fields: {}, is_default: draft!.banks.length === 0 }];
	}
	function setDefaultBank(i: number) {
		draft!.banks = draft!.banks.map((b, j) => ({ ...b, is_default: i === j }));
	}
</script>

<section class="card">
	<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3">
		<h2 class="font-medium text-slate-900">Requisites</h2>
		{#if canWrite}<button class="btn btn-ghost text-sm" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> Add</button>{/if}
	</div>
	<ul class="divide-y divide-slate-50">
		{#each items as r (r.id)}
			<li class="group px-5 py-3 text-sm">
				<div class="flex items-start gap-2">
					<div class="min-w-0 flex-1">
						<p class="font-medium text-slate-900">
							{r.name}
							{#if r.is_default && items.length > 1}<span class="badge ml-1 bg-brand-50 text-brand-700">Default</span>{/if}
						</p>
						<p class="text-xs text-slate-500">{countryName(r.country)} · {r.kind === 'person' ? 'Individual' : 'Organisation'}</p>
					</div>
					{#if canWrite}
						<div class="flex gap-1 opacity-0 group-hover:opacity-100 focus-within:opacity-100">
							{#if !r.is_default}<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => makeDefault(r)}>Make default</button>{/if}
							<button class="text-slate-400 hover:text-slate-800" aria-label="Edit requisites" onclick={() => edit(r)}><Icon name="edit" class="size-4" /></button>
							<button class="text-slate-300 hover:text-red-600" aria-label="Delete requisites" onclick={() => remove(r)}><Icon name="trash" class="size-4" /></button>
						</div>
					{/if}
				</div>
				<dl class="mt-2 grid grid-cols-[minmax(0,10rem)_1fr] gap-x-3 gap-y-0.5 text-xs">
					{#each ordered(r.country, r.fields) as [k, v] (k)}
						<dt class="truncate text-slate-500">{label(r.country, k)}</dt><dd class="font-mono text-slate-800">{v}</dd>
					{/each}
					{#if r.address}<dt class="text-slate-500">Legal address</dt><dd class="text-slate-800">{formatAddress(r.address)}</dd>{/if}
					{#each r.banks as b, i (b.id ?? i)}
						<dt class="truncate text-slate-500">{b.name || 'Bank account'}{r.banks.length > 1 && b.is_default ? ' (default)' : ''}</dt>
						<dd class="text-slate-800">{ordered(r.country, b.fields, true).map(([k, v]) => (k === 'bank_name' ? v : `${label(r.country, k, true)} ${v}`)).join(' · ')}</dd>
					{/each}
				</dl>
			</li>
		{:else}
			<li class="px-5 py-3 text-sm text-slate-400">No legal details yet. Add them to print tax numbers on quotes and invoices.</li>
		{/each}
	</ul>
</section>

<Modal bind:open title={draft?.id ? 'Edit requisites' : 'Add requisites'} size="lg">
	{#if draft}
		<div class="space-y-4 text-sm">
			<div class="grid gap-3 sm:grid-cols-2">
				<div>
					<label class="label" for="rq-country">Country</label>
					<select id="rq-country" class="input" value={draft.country} onchange={(e) => changeCountry(e.currentTarget.value)}>
						{#each countries as c (c.code)}<option value={c.code}>{c.name}</option>{/each}
					</select>
				</div>
				<div>
					<span class="label">Type</span>
					<div class="flex gap-4 pt-2">
						<label class="flex items-center gap-1.5"><input type="radio" class="text-brand-600" value="company" bind:group={draft.kind} /> Organisation</label>
						<label class="flex items-center gap-1.5"><input type="radio" class="text-brand-600" value="person" bind:group={draft.kind} /> Individual</label>
					</div>
				</div>
				<div class="sm:col-span-2">
					<label class="label" for="rq-name">{draft.kind === 'person' ? 'Full legal name' : 'Registered legal name'}</label>
					<input id="rq-name" class="input" maxlength="500" bind:value={draft.name} />
				</div>
				{#each visibleFields as f (f.code)}
					<div>
						<label class="label" for="rq-{f.code}">{f.label}</label>
						<input
							id="rq-{f.code}"
							class="input font-mono {errors[f.code] ? 'border-red-400' : ''}"
							placeholder={f.hint}
							maxlength="200"
							value={draft.fields[f.code] ?? ''}
							oninput={(e) => (draft!.fields[f.code] = e.currentTarget.value)}
							onblur={(e) => check(f.code, e.currentTarget.value)}
						/>
						{#if errors[f.code]}<p class="mt-1 text-xs text-red-600">{errors[f.code]}</p>{/if}
					</div>
				{/each}
			</div>

			{#if dups.length}
				<div class="rounded-lg bg-amber-50 p-3 text-xs text-amber-900" role="status">
					<p class="font-medium">Also used by:</p>
					<ul class="mt-1 space-y-0.5">
						{#each dups as d (d.item_id + d.field)}<li><a class="underline" href={itemLink(d.entity_type, d.item_id)} target="_blank" rel="noopener">{d.title}</a> — {d.field} {d.value}</li>{/each}
					</ul>
				</div>
			{/if}

			<div>
				<span class="label">Legal address <span class="font-normal text-slate-400">(if different from the record's address)</span></span>
				{#key draft.id}<AddressEditor bind:value={draft.address} />{/key}
			</div>

			<div class="space-y-3">
				<div class="flex items-center justify-between">
					<span class="label mb-0">Bank accounts</span>
					<button type="button" class="btn btn-ghost px-2 py-1 text-xs" onclick={addBank}><Icon name="plus" class="size-3.5" /> Add account</button>
				</div>
				{#each draft.banks as b, i (i)}
					<div class="rounded-lg border border-slate-200 p-3">
						<div class="mb-2 flex items-center gap-2">
							<input class="input flex-1" placeholder="Account name (optional)" bind:value={b.name} aria-label="Account name" />
							<label class="flex items-center gap-1 text-xs whitespace-nowrap"><input type="radio" class="text-brand-600" checked={b.is_default} onchange={() => setDefaultBank(i)} /> Default</label>
							<button type="button" class="text-slate-300 hover:text-red-600" aria-label="Remove account" onclick={() => (draft!.banks = draft!.banks.filter((_, j) => j !== i))}><Icon name="x" class="size-4" /></button>
						</div>
						<div class="grid gap-2 sm:grid-cols-2">
							{#each schema?.bank ?? [] as f (f.code)}
								<div>
									<input
										class="input font-mono text-xs {errors[`b${i}.${f.code}`] ? 'border-red-400' : ''}"
										placeholder={f.hint ? `${f.label} — ${f.hint}` : f.label}
										aria-label={f.label}
										value={b.fields[f.code] ?? ''}
										oninput={(e) => (b.fields[f.code] = e.currentTarget.value)}
										onblur={(e) => check(f.code, e.currentTarget.value, `b${i}.${f.code}`)}
									/>
									{#if errors[`b${i}.${f.code}`]}<p class="mt-1 text-xs text-red-600">{errors[`b${i}.${f.code}`]}</p>{/if}
								</div>
							{/each}
						</div>
					</div>
				{/each}
			</div>

			{#if draft.id === null || !draft.is_default}
				<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.is_default} /> Use by default on documents</label>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" disabled={saving || !draft?.name.trim()} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
	{/snippet}
</Modal>
