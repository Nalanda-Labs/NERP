<script lang="ts">
	// A bill: edited while a draft, then posted, paid or voided.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, downloadUrl } from '#lib/api.ts';
	import { amount, baseCurrency, STATUS_BADGE, taxes, today, type Party, type Tax } from '#lib/accounting.ts';
	import AccountSelect from '#lib/components/accounting/AccountSelect.svelte';
	import PartyPicker from '#lib/components/accounting/PartyPicker.svelte';
	import PaymentModal from '#lib/components/accounting/PaymentModal.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Line = { description: string; account_id: string; quantity: string; unit_price: string; tax_id: string; net?: string; tax?: string; account_code?: string; account_name?: string; tax_name?: string | null };
	type Bill = {
		id: string; number: number | null; reference: string | null; party_id: string | null; party_name: string | null; date: string; due_date: string | null;
		currency: string; fx_rate: string; tax_inclusive: boolean; subtotal: string; tax: string; total: string; status: string; entry_id: string | null; entry_number: number | null;
		memo: string | null; paid: string; balance: string; files: { id: string; name: string; size: number }[]; lines: Line[]; payments: { id: string; number: number; date: string; amount: string; status: string }[];
	};
	const id = $derived(page.params.id ?? 'new');
	let bill = $state<Bill | null>(null);
	let party = $state<Party | null>(null);
	let form = $state({ reference: '', date: today(), due_date: '', currency: '', fx_rate: '', tax_inclusive: false, memo: '' });
	let lines = $state<Line[]>([]);
	let files = $state<{ id: string; name: string }[]>([]);
	let busy = $state(false);
	let taxList = $state<Tax[]>([]);
	const canPost = session.can(PERM.ACCOUNTING_POST) || session.can(PERM.ACCOUNTING_ADMIN);
	const editable = $derived(canPost && (!bill || bill.status === 'draft'));

	const blank = (): Line => ({ description: '', account_id: '', quantity: '1', unit_price: '', tax_id: '' });
	$effect(() => {
		id;
		taxes().then((t) => (taxList = t.filter((x) => x.scope === 'purchases')));
		if (id === 'new') {
			bill = null;
			party = null;
			baseCurrency().then((c) => (form = { reference: '', date: today(), due_date: '', currency: c, fx_rate: '', tax_inclusive: false, memo: '' }));
			lines = [blank()];
			files = [];
		} else load();
	});
	async function load() {
		const b = await api.get<Bill>(`/accounting/bills/${id}`);
		bill = b;
		party = b.party_id ? { id: b.party_id, name: b.party_name ?? '', kind: 'company' } : null;
		form = { reference: b.reference ?? '', date: b.date, due_date: b.due_date ?? '', currency: b.currency, fx_rate: String(Number(b.fx_rate)), tax_inclusive: b.tax_inclusive, memo: b.memo ?? '' };
		lines = b.lines.map((l) => ({ ...l, quantity: String(Number(l.quantity)), unit_price: String(Number(l.unit_price)), tax_id: l.tax_id ?? '' }));
		files = b.files.map((f) => ({ id: f.id, name: f.name }));
	}

	// A preview of the totals (the server works them out for good).
	const preview = $derived.by(() => {
		let net = 0, tax = 0;
		for (const l of lines) {
			const amt = (Number(l.quantity) || 0) * (Number(l.unit_price) || 0);
			const rate = Number(taxList.find((t) => t.id === l.tax_id)?.rate ?? 0) / 100;
			if (form.tax_inclusive) {
				const n = Math.round((amt / (1 + rate)) * 100) / 100;
				net += n;
				tax += Math.round(amt * 100) / 100 - n;
			} else {
				const n = Math.round(amt * 100) / 100;
				net += n;
				tax += Math.round(n * rate * 100) / 100;
			}
		}
		return { net, tax, total: net + tax };
	});

	async function upload(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		for (const f of Array.from(input.files ?? [])) {
			const fd = new FormData();
			fd.append('file', f);
			try {
				const up = await api.post<{ id: string }>('/files', fd);
				files = [...files, { id: up.id, name: f.name }];
			} catch (err) {
				toasts.error(err);
			}
		}
		input.value = '';
	}
	const body = () => ({
		party_id: party?.id ?? null,
		reference: form.reference || null,
		date: form.date,
		due_date: form.due_date || null,
		currency: form.currency,
		fx_rate: form.fx_rate && form.fx_rate !== '1' ? form.fx_rate : null,
		tax_inclusive: form.tax_inclusive,
		memo: form.memo || null,
		file_ids: files.map((f) => f.id),
		lines: lines.map((l) => ({ description: l.description, account_id: l.account_id, quantity: l.quantity || '1', unit_price: l.unit_price || '0', tax_id: l.tax_id || null }))
	});
	async function save(post = false) {
		busy = true;
		try {
			let b: Bill;
			if (id === 'new') b = await api.post<Bill>('/accounting/bills', body());
			else b = await api.put<Bill>(`/accounting/bills/${id}`, body());
			if (post) b = await api.post<Bill>(`/accounting/bills/${b.id}/post`, {});
			toasts.success(post ? `Bill ${b.number} posted` : 'Draft saved');
			if (id === 'new') goto(`/accounting/bills/${b.id}`, { replaceState: true });
			else load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function remove() {
		if (!confirm('Delete this draft bill?')) return;
		await api.del(`/accounting/bills/${id}`);
		goto('/accounting/bills?status=draft');
	}
	async function voidIt() {
		if (!confirm('Void this bill? A reversing entry is posted.')) return;
		try {
			await api.post(`/accounting/bills/${id}/void`, {});
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
	let payOpen = $state(false);
	const cur = $derived(form.currency);
</script>

<svelte:head><title>{bill?.number ? `Bill ${bill.number}` : 'New bill'} · Accounting · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<a href="/accounting/bills" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Bills</a>
	<div class="flex flex-wrap items-start justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">{bill?.number ? `Bill ${bill.number}` : bill ? 'Draft bill' : 'New bill'}</h1>
			{#if bill}<p class="text-sm text-slate-500"><span class="badge {STATUS_BADGE[bill.status]} capitalize">{bill.status}</span>{#if bill.entry_number}<a class="ml-2 hover:underline" href="/accounting/journal/{bill.entry_id}">Entry {bill.entry_number}</a>{/if}</p>{/if}
		</div>
		<div class="flex flex-wrap gap-2">
			{#if editable}
				{#if bill}<button class="btn btn-ghost text-red-600" onclick={remove}>Delete</button>{/if}
				<button class="btn btn-secondary" disabled={busy} onclick={() => save(false)}>Save draft</button>
				<button class="btn btn-primary" disabled={busy} onclick={() => save(true)}>Post</button>
			{:else if bill && canPost}
				{#if bill.status === 'open'}<button class="btn btn-primary" onclick={() => (payOpen = true)}>Pay</button>{/if}
				{#if (bill.status === 'open' || bill.status === 'paid') && Number(bill.paid) === 0}<button class="btn btn-ghost text-red-600" onclick={voidIt}>Void</button>{/if}
			{/if}
		</div>
	</div>

	<div class="card grid gap-4 p-5 sm:grid-cols-2 lg:grid-cols-4">
		<div class="sm:col-span-2">
			<span class="label">Supplier</span>
			{#if editable}<PartyPicker bind:value={party} label="Supplier" />{:else}<p class="text-sm font-medium">{party?.name ?? '—'}</p>{/if}
		</div>
		<div><label class="label" for="b-ref">Their invoice number</label><input id="b-ref" class="input" disabled={!editable} bind:value={form.reference} /></div>
		<div><label class="label" for="b-cur">Currency</label><input id="b-cur" class="input uppercase" maxlength="3" disabled={!editable} bind:value={form.currency} /></div>
		<div><label class="label" for="b-date">Date</label><input id="b-date" type="date" class="input" disabled={!editable} bind:value={form.date} /></div>
		<div><label class="label" for="b-due">Due</label><input id="b-due" type="date" class="input" disabled={!editable} bind:value={form.due_date} /></div>
		<div><label class="label" for="b-fx">Exchange rate</label><input id="b-fx" class="input" inputmode="decimal" placeholder="The CRM's" disabled={!editable} bind:value={form.fx_rate} /></div>
		<label class="flex items-end gap-2 pb-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" disabled={!editable} bind:checked={form.tax_inclusive} /> Prices include tax</label>
	</div>

	<div class="card overflow-x-auto">
		<table class="w-full text-sm" aria-label="Bill lines">
			<thead class="border-b border-slate-100 text-left text-xs text-slate-500 uppercase">
				<tr><th class="px-3 py-2">What</th><th class="px-3 py-2">Account</th><th class="w-24 px-3 py-2 text-right">Qty</th><th class="w-32 px-3 py-2 text-right">Price</th><th class="px-3 py-2">Tax</th>{#if !editable}<th class="px-3 py-2 text-right">Net</th><th class="px-3 py-2 text-right">Tax</th>{/if}<th></th></tr>
			</thead>
			<tbody class="divide-y divide-slate-100">
				{#each lines as l, i (i)}
					<tr>
						{#if editable}
							<td class="px-3 py-2"><input class="input" aria-label="Line {i + 1} description" bind:value={l.description} /></td>
							<td class="min-w-56 px-3 py-2"><AccountSelect label="Line {i + 1} account" types={['expense', 'asset']} bind:value={l.account_id} /></td>
							<td class="px-3 py-2"><input class="input text-right" inputmode="decimal" aria-label="Line {i + 1} quantity" bind:value={l.quantity} /></td>
							<td class="px-3 py-2"><input class="input text-right" inputmode="decimal" aria-label="Line {i + 1} price" bind:value={l.unit_price} /></td>
							<td class="px-3 py-2"><select class="input" aria-label="Line {i + 1} tax" bind:value={l.tax_id}><option value="">No tax</option>{#each taxList.filter((t) => t.is_active) as t (t.id)}<option value={t.id}>{t.name} ({Number(t.rate)}%)</option>{/each}</select></td>
							<td class="px-3 py-2">{#if lines.length > 1}<button class="text-slate-400 hover:text-red-600" aria-label="Remove line {i + 1}" onclick={() => lines.splice(i, 1)}><Icon name="x" class="size-4" /></button>{/if}</td>
						{:else}
							<td class="px-3 py-2">{l.description}</td>
							<td class="px-3 py-2">{l.account_code} · {l.account_name}</td>
							<td class="px-3 py-2 text-right">{l.quantity}</td>
							<td class="px-3 py-2 text-right">{amount(l.unit_price, cur)}</td>
							<td class="px-3 py-2">{l.tax_name ?? ''}</td>
							<td class="px-3 py-2 text-right">{amount(l.net ?? 0, cur)}</td>
							<td class="px-3 py-2 text-right">{amount(l.tax ?? 0, cur)}</td>
							<td></td>
						{/if}
					</tr>
				{/each}
			</tbody>
		</table>
		<div class="flex flex-wrap items-start justify-between gap-4 border-t border-slate-100 p-4">
			{#if editable}<button class="text-sm text-brand-600 hover:underline" onclick={() => lines.push(blank())}>+ Line</button>{:else}<span></span>{/if}
			<dl class="grid grid-cols-2 gap-x-6 gap-y-1 text-sm" data-totals>
				<dt class="text-slate-500">Net</dt><dd class="text-right">{amount(bill && !editable ? bill.subtotal : preview.net, cur)}</dd>
				<dt class="text-slate-500">Tax</dt><dd class="text-right">{amount(bill && !editable ? bill.tax : preview.tax, cur)}</dd>
				<dt class="font-medium">Total</dt><dd class="text-right font-semibold" data-total>{amount(bill && !editable ? bill.total : preview.total, cur)}</dd>
				{#if bill && !editable && bill.status !== 'void'}<dt class="text-slate-500">Left to pay</dt><dd class="text-right">{amount(bill.balance, cur)}</dd>{/if}
			</dl>
		</div>
	</div>

	<div class="grid gap-4 md:grid-cols-2">
		<div class="card space-y-2 p-5">
			<h2 class="font-semibold text-slate-900">Attachments</h2>
			<ul class="space-y-1 text-sm">
				{#each files as f, i (f.id)}<li class="flex items-center gap-2"><Icon name="paperclip" class="size-4 text-slate-400" /><button class="flex-1 truncate text-left text-brand-700 hover:underline" onclick={() => downloadUrl(`/api/files/${f.id}/content`, f.name)}>{f.name}</button>{#if editable}<button aria-label="Remove {f.name}" class="text-slate-400 hover:text-red-600" onclick={() => files.splice(i, 1)}><Icon name="x" class="size-4" /></button>{/if}</li>{:else}<li class="text-slate-500">The supplier's invoice, as a PDF or a photo.</li>{/each}
			</ul>
			{#if editable}<label class="btn btn-ghost cursor-pointer px-2 text-sm"><Icon name="upload" class="size-4" /> Attach<input type="file" multiple class="sr-only" onchange={upload} /></label>{/if}
			<label class="label mt-3" for="b-memo">Notes</label><textarea id="b-memo" class="input" rows="2" disabled={!editable} bind:value={form.memo}></textarea>
		</div>
		{#if bill?.payments.length}
			<div class="card p-5">
				<h2 class="mb-2 font-semibold text-slate-900">Payments</h2>
				<ul class="space-y-1 text-sm">{#each bill.payments as p (p.id)}<li class="flex justify-between {p.status === 'void' ? 'text-slate-400 line-through' : ''}"><a class="hover:underline" href="/accounting/payments?id={p.id}">Payment {p.number} · {formatDate(p.date)}</a><span>{amount(p.amount, cur)}</span></li>{/each}</ul>
			</div>
		{/if}
	</div>
</div>

{#if bill}<PaymentModal bind:open={payOpen} direction="out" party={party} docId={bill.id} onsaved={load} />{/if}
