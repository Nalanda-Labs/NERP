<script lang="ts">
	// One journal entry: written as a draft, posted, or reversed.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import { amount, baseCurrency, STATUS_BADGE, today, type Party } from '#lib/accounting.ts';
	import AccountSelect from '#lib/components/accounting/AccountSelect.svelte';
	import PartyPicker from '#lib/components/accounting/PartyPicker.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDate, formatDateTime } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Line = { account_id: string; debit: string; credit: string; description: string; party: Party | null; account_code?: string; account_name?: string; party_name?: string | null; currency?: string | null; amount_currency?: string | null; tax_name?: string | null };
	type Entry = { id: string; number: number | null; date: string; memo: string | null; source: string; source_id: string | null; status: string; reverses: string | null; reverses_number: number | null; reversed_by: string | null; reversed_by_number: number | null; created_by_name: string | null; created_at: string; lines: (Omit<Line, 'party'> & { party_id: string | null })[] };
	const id = $derived(page.params.id ?? 'new');
	let entry = $state<Entry | null>(null);
	let date = $state(today());
	let memo = $state('');
	let lines = $state<Line[]>([]);
	let busy = $state(false);
	let cur = $state('');
	baseCurrency().then((c) => (cur = c));
	const canPost = session.can(PERM.ACCOUNTING_POST) || session.can(PERM.ACCOUNTING_ADMIN);
	const editable = $derived(canPost && (!entry || entry.status === 'draft'));
	const blank = (): Line => ({ account_id: '', debit: '', credit: '', description: '', party: null });

	$effect(() => {
		id;
		if (id === 'new') {
			entry = null;
			date = today();
			memo = '';
			lines = [blank(), blank()];
		} else load();
	});
	async function load() {
		const e = await api.get<Entry>(`/accounting/entries/${id}`);
		entry = e;
		date = e.date;
		memo = e.memo ?? '';
		lines = e.lines.map((l) => ({ ...l, debit: Number(l.debit) ? String(Number(l.debit)) : '', credit: Number(l.credit) ? String(Number(l.credit)) : '', description: l.description ?? '', party: l.party_id ? { id: l.party_id, name: l.party_name ?? '', kind: 'company' } : null }));
	}
	const totals = $derived({ debit: lines.reduce((t, l) => t + (Number(l.debit) || 0), 0), credit: lines.reduce((t, l) => t + (Number(l.credit) || 0), 0) });
	const diff = $derived(Math.round((totals.debit - totals.credit) * 100) / 100);
	const body = (post: boolean) => ({
		date,
		memo: memo || null,
		post,
		lines: lines.filter((l) => l.account_id).map((l) => ({ account_id: l.account_id, debit: (Number(l.debit) || 0).toFixed(2), credit: (Number(l.credit) || 0).toFixed(2), description: l.description || null, party_id: l.party?.id ?? null }))
	});
	async function save(post: boolean) {
		busy = true;
		try {
			const e = id === 'new' ? await api.post<Entry>('/accounting/entries', body(post)) : await api.put<Entry>(`/accounting/entries/${id}`, body(post));
			toasts.success(post ? `Entry ${e.number} posted` : 'Draft saved');
			if (id === 'new') goto(`/accounting/journal/${e.id}`, { replaceState: true });
			else load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function remove() {
		if (!confirm('Delete this draft?')) return;
		await api.del(`/accounting/entries/${id}`);
		goto('/accounting/journal');
	}
	let revDate = $state(today());
	async function reverse() {
		if (!confirm(`Reverse entry ${entry!.number} on ${formatDate(revDate)}? A new entry with every line the other way round is posted.`)) return;
		try {
			await api.post(`/accounting/entries/${id}/reverse`, { date: revDate });
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
	const sourceLink = $derived.by(() => {
		if (!entry?.source_id) return null;
		if (entry.source === 'invoice') return { href: `/crm/invoice/${entry.source_id}`, label: 'the invoice' };
		if (entry.source === 'bill') return { href: `/accounting/bills/${entry.source_id}`, label: 'the bill' };
		if (entry.source === 'payment') return { href: `/accounting/payments?id=${entry.source_id}`, label: 'the payment' };
		return null;
	});
</script>

<svelte:head><title>{entry?.number ? `Entry ${entry.number}` : 'Journal entry'} · Accounting · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<a href="/accounting/journal" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Journal</a>
	<div class="flex flex-wrap items-start justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">{entry?.number ? `Entry ${entry.number}` : entry ? 'Draft entry' : 'New journal entry'}</h1>
			{#if entry}
				<p class="mt-1 space-x-2 text-sm text-slate-500">
					<span class="badge {STATUS_BADGE[entry.status]}" data-entry-status={entry.status}>{entry.status === 'void' ? 'reversed' : entry.status}</span>
					{#if entry.reversed_by}<a class="hover:underline" href="/accounting/journal/{entry.reversed_by}">reversed by entry {entry.reversed_by_number}</a>{/if}
					{#if entry.reverses}<a class="hover:underline" href="/accounting/journal/{entry.reverses}">reverses entry {entry.reverses_number}</a>{/if}
					{#if sourceLink}<a class="hover:underline" href={sourceLink.href}>from {sourceLink.label}</a>{/if}
					<span>· {entry.created_by_name ?? ''} {formatDateTime(entry.created_at)}</span>
				</p>
			{/if}
		</div>
		<div class="flex flex-wrap items-center gap-2">
			{#if editable}
				{#if entry}<button class="btn btn-ghost text-red-600" onclick={remove}>Delete draft</button>{/if}
				<button class="btn btn-secondary" disabled={busy} onclick={() => save(false)}>Save draft</button>
				<button class="btn btn-primary" disabled={busy || diff !== 0 || totals.debit === 0} onclick={() => save(true)}>Post</button>
			{:else if entry?.status === 'posted' && canPost && ['manual', 'opening', 'bank', 'closing'].includes(entry.source)}
				<input type="date" class="input w-40" aria-label="Reversal date" bind:value={revDate} />
				<button class="btn btn-secondary" onclick={reverse}>Reverse</button>
			{/if}
		</div>
	</div>

	<div class="card grid gap-4 p-5 sm:grid-cols-4">
		<div><label class="label" for="je-date">Date</label><input id="je-date" type="date" class="input" disabled={!editable} bind:value={date} /></div>
		<div class="sm:col-span-3"><label class="label" for="je-memo">Memo</label><input id="je-memo" class="input" disabled={!editable} bind:value={memo} /></div>
	</div>

	<div class="card overflow-x-auto">
		<table class="w-full text-sm" aria-label="Entry lines">
			<thead class="border-b border-slate-100 text-left text-xs text-slate-500 uppercase"><tr><th class="px-3 py-2">Account</th><th class="px-3 py-2">Description</th><th class="px-3 py-2">Customer / supplier</th><th class="w-32 px-3 py-2 text-right">Debit</th><th class="w-32 px-3 py-2 text-right">Credit</th><th></th></tr></thead>
			<tbody class="divide-y divide-slate-100">
				{#each lines as l, i (i)}
					<tr>
						{#if editable}
							<td class="min-w-56 px-3 py-2"><AccountSelect label="Line {i + 1} account" bind:value={l.account_id} /></td>
							<td class="px-3 py-2"><input class="input" aria-label="Line {i + 1} description" bind:value={l.description} /></td>
							<td class="min-w-48 px-3 py-2"><PartyPicker bind:value={l.party} label="Line {i + 1} customer or supplier" placeholder="Optional" /></td>
							<td class="px-3 py-2"><input class="input text-right" inputmode="decimal" aria-label="Line {i + 1} debit" bind:value={l.debit} oninput={() => l.debit && (l.credit = '')} /></td>
							<td class="px-3 py-2"><input class="input text-right" inputmode="decimal" aria-label="Line {i + 1} credit" bind:value={l.credit} oninput={() => l.credit && (l.debit = '')} /></td>
							<td class="px-3 py-2">{#if lines.length > 2}<button class="text-slate-400 hover:text-red-600" aria-label="Remove line {i + 1}" onclick={() => lines.splice(i, 1)}><Icon name="x" class="size-4" /></button>{/if}</td>
						{:else}
							<td class="px-3 py-2"><a class="hover:text-brand-700" href="/accounting/reports?tab=ledger&account={l.account_id}">{l.account_code} · {l.account_name}</a></td>
							<td class="px-3 py-2">{l.description}{#if l.tax_name}<span class="ml-1 text-xs text-slate-500">({l.tax_name})</span>{/if}{#if l.currency}<span class="ml-1 text-xs text-slate-500">{amount(l.amount_currency, l.currency)}</span>{/if}</td>
							<td class="px-3 py-2">{l.party?.name ?? ''}</td>
							<td class="px-3 py-2 text-right">{Number(l.debit) ? amount(l.debit, cur) : ''}</td>
							<td class="px-3 py-2 text-right">{Number(l.credit) ? amount(l.credit, cur) : ''}</td>
							<td></td>
						{/if}
					</tr>
				{/each}
			</tbody>
			<tfoot class="border-t border-slate-200 font-medium">
				<tr>
					<td class="px-3 py-2">{#if editable}<button class="text-sm font-normal text-brand-600 hover:underline" onclick={() => lines.push(blank())}>+ Line</button>{/if}</td>
					<td colspan="2" class="px-3 py-2 text-right {diff !== 0 ? 'text-red-600' : 'text-slate-500'}" data-difference>{diff !== 0 ? `Out of balance by ${amount(Math.abs(diff), cur)}` : 'Balanced'}</td>
					<td class="px-3 py-2 text-right">{amount(totals.debit, cur)}</td>
					<td class="px-3 py-2 text-right">{amount(totals.credit, cur)}</td>
					<td></td>
				</tr>
			</tfoot>
		</table>
	</div>
</div>
