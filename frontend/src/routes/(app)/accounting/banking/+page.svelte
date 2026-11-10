<script lang="ts">
	// Bank statements: import, then account for every line.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import { accounts, amount, taxes, type Account, type Party, type Tax } from '#lib/accounting.ts';
	import AccountSelect from '#lib/components/accounting/AccountSelect.svelte';
	import PartyPicker from '#lib/components/accounting/PartyPicker.svelte';
	import PaymentModal from '#lib/components/accounting/PaymentModal.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Suggestion = { line_id: string; date: string; amount: string; entry_number: number | null; memo: string | null; party_name: string | null };
	type BankLine = { id: string; date: string; description: string; reference: string | null; amount: string; status: string; line_id: string | null; entry_number: number | null; suggestions: Suggestion[] };
	type Summary = { account_id: string; name: string; currency: string; book_balance: string; statement_lines: number; unmatched: number; unmatched_amount: string; last_statement_date: string | null; unreconciled: (Suggestion & { entry_id: string })[] };

	let banks = $state<Account[]>([]);
	const account = $derived(page.url.searchParams.get('account') ?? banks[0]?.id ?? '');
	const view = $derived(page.url.searchParams.get('view') ?? 'unmatched');
	let summary = $state<Summary | null>(null);
	let lines = $state<BankLine[]>([]);
	let taxList = $state<Tax[]>([]);
	const canPost = session.can(PERM.ACCOUNTING_POST) || session.can(PERM.ACCOUNTING_ADMIN);

	$effect(() => {
		accounts(true).then((a) => (banks = a.filter((x) => (x.subtype === 'bank' || x.subtype === 'cash') && x.is_active)));
		taxes().then((t) => (taxList = t.filter((x) => x.is_active)));
	});
	async function load() {
		if (!account) return;
		[summary, lines] = await Promise.all([
			api.get<Summary>(`/accounting/bank/${account}`),
			api.get<{ items: BankLine[] }>(`/accounting/bank/${account}/lines` + query({ status: view })).then((r) => r.items)
		]);
	}
	$effect(() => {
		account;
		view;
		load();
	});
	const go = (p: Record<string, string>) => goto('?' + new URLSearchParams({ account, view, ...p }).toString(), { replaceState: true });

	// Import
	let order = $state('');
	let importing = $state(false);
	async function importFile(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const f = input.files?.[0];
		if (!f) return;
		importing = true;
		try {
			const fd = new FormData();
			fd.append('file', f);
			const up = await api.post<{ id: string }>('/files', fd);
			const r = await api.post<{ added: number; skipped: number; date_order: string | null }>(`/accounting/bank/${account}/import`, { file_id: up.id, date_order: order || null });
			toasts.success(`${r.added} line${r.added === 1 ? '' : 's'} added${r.skipped ? `, ${r.skipped} already imported` : ''}`);
			load();
		} catch (err) {
			toasts.error(err);
		} finally {
			importing = false;
			input.value = '';
		}
	}

	async function act(path: string, body: Record<string, unknown> = {}) {
		try {
			await api.post(path, body);
			load();
		} catch (e) {
			toasts.error(e);
		}
	}

	// Recording a line as a new entry
	let recordFor = $state<BankLine | null>(null);
	let rec = $state({ account_id: '', tax_id: '', description: '' });
	let recParty = $state<Party | null>(null);
	async function record(e: SubmitEvent) {
		e.preventDefault();
		try {
			await api.post(`/accounting/bank/lines/${recordFor!.id}/record`, { account_id: rec.account_id, tax_id: rec.tax_id || null, party_id: recParty?.id ?? null, description: rec.description || null });
			recordFor = null;
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
	// As a customer's or supplier's payment
	let payFor = $state<BankLine | null>(null);
	let payOpen = $state(false);
	const cur = $derived(summary?.currency ?? '');
</script>

<svelte:head><title>Banking · Accounting · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Banking</h1>
			<p class="text-sm text-slate-500">Import statements and account for every line: match it, record it, or ignore it.</p>
		</div>
		<select class="input w-72" aria-label="Bank account" value={account} onchange={(e) => go({ account: e.currentTarget.value })}>
			{#each banks as b (b.id)}<option value={b.id}>{b.code} · {b.name}{b.currency ? ` (${b.currency})` : ''}</option>{/each}
		</select>
	</div>

	{#if !banks.length}
		<div class="card p-10 text-center text-slate-500">Add a bank or cash account in the chart of accounts first.</div>
	{:else if summary}
		<div class="grid gap-4 sm:grid-cols-3">
			<div class="card p-5"><p class="text-sm text-slate-500">In the books</p><p class="mt-1 text-2xl font-semibold" data-book-balance>{amount(summary.book_balance, cur)}</p></div>
			<div class="card p-5"><p class="text-sm text-slate-500">Statement lines to reconcile</p><p class="mt-1 text-2xl font-semibold" data-unmatched>{summary.unmatched}</p>{#if summary.last_statement_date}<p class="text-xs text-slate-500">Statement up to {formatDate(summary.last_statement_date)}</p>{/if}</div>
			{#if canPost}
				<div class="card space-y-2 p-5">
					<p class="text-sm text-slate-500">Import a statement (CSV or OFX)</p>
					<div class="flex gap-2">
						<select class="input w-36" aria-label="Date order" bind:value={order}><option value="">Dates: detect</option><option value="dmy">Day first</option><option value="mdy">Month first</option></select>
						<label class="btn btn-primary cursor-pointer {importing ? 'opacity-60' : ''}"><Icon name="upload" class="size-4" /> Import<input type="file" accept=".csv,.ofx,.qfx,.txt" class="sr-only" aria-label="Statement file" onchange={importFile} /></label>
					</div>
				</div>
			{/if}
		</div>

		<div class="flex gap-1 border-b border-slate-200" role="tablist">
			{#each [['unmatched', 'To reconcile'], ['matched', 'Matched'], ['ignored', 'Ignored'], ['books', 'Only in the books']] as [k, label] (k)}
				<button role="tab" aria-selected={view === k} class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {view === k ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500 hover:text-slate-800'}" onclick={() => go({ view: k })}>{label}</button>
			{/each}
		</div>

		{#if view === 'books'}
			<div class="card">
				<p class="border-b border-slate-100 px-4 py-3 text-sm text-slate-500">Entries on this account that no statement line accounts for yet.</p>
				<ul class="divide-y divide-slate-100 text-sm">
					{#each summary.unreconciled as u (u.line_id)}
						<li class="flex items-center gap-3 px-4 py-2"><span class="w-24 text-slate-500">{formatDate(u.date)}</span><a class="flex-1 truncate hover:text-brand-700" href="/accounting/journal/{u.entry_id}">Entry {u.entry_number} · {u.memo ?? ''}</a><span class="text-right">{amount(u.amount, cur)}</span></li>
					{:else}
						<li class="px-4 py-8 text-center text-slate-500">Everything in the books is on a statement.</li>
					{/each}
				</ul>
			</div>
		{:else}
			<div class="card">
				<ul class="divide-y divide-slate-100">
					{#each lines as l (l.id)}
						<li class="flex flex-wrap items-center gap-3 px-4 py-3 text-sm" data-bank-line={l.description}>
							<span class="w-24 text-slate-500">{formatDate(l.date)}</span>
							<span class="min-w-0 flex-1"><span class="block truncate font-medium text-slate-800">{l.description || '—'}</span>{#if l.reference}<span class="text-xs text-slate-500">{l.reference}</span>{/if}</span>
							<span class="w-32 text-right font-medium {Number(l.amount) > 0 ? 'text-emerald-700' : ''}">{amount(l.amount, cur)}</span>
							<span class="flex flex-wrap justify-end gap-1">
								{#if l.status === 'unmatched' && canPost}
									{#each l.suggestions as s (s.line_id)}<button class="btn btn-secondary py-1 text-xs" onclick={() => act(`/accounting/bank/lines/${l.id}/match`, { line_id: s.line_id })}>Match entry {s.entry_number}{s.party_name ? ` · ${s.party_name}` : ''}</button>{/each}
									<button class="btn btn-ghost py-1 text-xs" onclick={() => ((payFor = l), (payOpen = true))}>{Number(l.amount) > 0 ? 'Customer payment' : 'Supplier payment'}</button>
									<button class="btn btn-ghost py-1 text-xs" onclick={() => ((recordFor = l), (rec = { account_id: '', tax_id: '', description: l.description }), (recParty = null))}>Record…</button>
									<button class="btn btn-ghost py-1 text-xs text-slate-500" onclick={() => act(`/accounting/bank/lines/${l.id}/ignore`)}>Ignore</button>
								{:else if l.status === 'matched'}
									<span class="text-xs text-slate-500">Entry {l.entry_number}</span>
									{#if canPost}<button class="btn btn-ghost py-1 text-xs" onclick={() => act(`/accounting/bank/lines/${l.id}/unmatch`)}>Undo match</button>{/if}
								{:else if l.status === 'ignored' && canPost}
									<button class="btn btn-ghost py-1 text-xs" onclick={() => act(`/accounting/bank/lines/${l.id}/unmatch`)}>Bring back</button>
								{/if}
							</span>
						</li>
					{:else}
						<li class="px-4 py-10 text-center text-slate-500">{view === 'unmatched' ? 'Nothing to reconcile.' : 'Nothing here.'}</li>
					{/each}
				</ul>
			</div>
		{/if}
	{/if}
</div>

<Modal open={!!recordFor} title="Record a statement line" onclose={() => (recordFor = null)}>
	{#if recordFor}
		<form id="record-line" class="space-y-3" onsubmit={record}>
			<p class="text-sm text-slate-600">{formatDate(recordFor.date)} · {recordFor.description} · <span class="font-medium">{amount(recordFor.amount, cur)}</span></p>
			<div><label class="label" for="rec-account">{Number(recordFor.amount) > 0 ? 'Money from (income, a loan, another account…)' : 'Money for (an expense, an asset, another account…)'}</label><AccountSelect id="rec-account" required bind:value={rec.account_id} /></div>
			<div>
				<label class="label" for="rec-tax">Tax included in the amount</label>
				<select id="rec-tax" class="input" bind:value={rec.tax_id}><option value="">None</option>{#each taxList.filter((t) => t.scope === (Number(recordFor!.amount) > 0 ? 'sales' : 'purchases')) as t (t.id)}<option value={t.id}>{t.name} ({Number(t.rate)}%)</option>{/each}</select>
			</div>
			<div><span class="label">Who <span class="font-normal text-slate-400">(optional)</span></span><PartyPicker bind:value={recParty} /></div>
			<div><label class="label" for="rec-desc">Description</label><input id="rec-desc" class="input" bind:value={rec.description} /></div>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (recordFor = null)}>Cancel</button>
		<button class="btn btn-primary" form="record-line">Record</button>
	{/snippet}
</Modal>

{#if payFor}<PaymentModal bind:open={payOpen} direction={Number(payFor.amount) > 0 ? 'in' : 'out'} bankLine={{ id: payFor.id, amount: payFor.amount, date: payFor.date, account_id: account, currency: cur }} onsaved={load} />{/if}
