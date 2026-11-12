<script lang="ts">
	// Every journal entry, posted by hand or by documents.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import { amount, baseCurrency, STATUS_BADGE } from '#lib/accounting.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Row = { id: string; number: number | null; date: string; memo: string | null; source: string; status: string; amount: string; reverses: string | null };
	const SOURCES: Record<string, string> = { manual: 'Manual', invoice: 'Invoice', bill: 'Bill', payment: 'Payment', bank: 'Bank', opening: 'Opening', closing: 'Year-end', inventory: 'Stock' };
	let rows = $state<Row[]>([]);
	let more = $state(false);
	let f = $state({ from: '', to: '', source: '', status: page.url.searchParams.get('status') ?? '', q: '' });
	let cur = $state('');
	baseCurrency().then((c) => (cur = c));
	async function load(append = false) {
		const r = await api.get<{ items: Row[]; more: boolean }>('/accounting/entries' + query({ from: f.from || undefined, to: f.to || undefined, source: f.source || undefined, status: f.status || undefined, q: f.q || undefined, offset: append ? rows.length : 0, limit: 100 }));
		rows = append ? [...rows, ...r.items] : r.items;
		more = r.more;
	}
	$effect(() => {
		f.from; f.to; f.source; f.status;
		load();
	});
	let t: ReturnType<typeof setTimeout> | undefined;
	const search = () => (clearTimeout(t), (t = setTimeout(() => load(), 250)));
</script>

<svelte:head><title>Journal · Accounting · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Journal</h1>
			<p class="text-sm text-slate-500">Every entry in the books. Posted entries never change: corrections are reversals.</p>
		</div>
		{#if session.can(PERM.ACCOUNTING_POST) || session.can(PERM.ACCOUNTING_ADMIN)}<a class="btn btn-primary" href="/accounting/journal/new"><Icon name="plus" class="size-4" /> New entry</a>{/if}
	</div>
	<div class="flex flex-wrap gap-2 text-sm">
		<input type="date" class="input w-40" aria-label="From" bind:value={f.from} />
		<input type="date" class="input w-40" aria-label="To" bind:value={f.to} />
		<select class="input w-36" aria-label="Source" bind:value={f.source}><option value="">All sources</option>{#each Object.entries(SOURCES) as [k, v] (k)}<option value={k}>{v}</option>{/each}</select>
		<select class="input w-32" aria-label="Status" bind:value={f.status}><option value="">Any status</option><option value="draft">Drafts</option><option value="posted">Posted</option><option value="void">Reversed</option></select>
		<input class="input w-56" type="search" placeholder="Memo or number" aria-label="Search entries" bind:value={f.q} oninput={search} />
	</div>
	<div class="card overflow-x-auto">
		<table class="w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs text-slate-500 uppercase"><tr><th class="px-4 py-2">No.</th><th class="px-4 py-2">Date</th><th class="px-4 py-2">Memo</th><th class="px-4 py-2">Source</th><th class="px-4 py-2 text-right">Amount</th><th class="px-4 py-2">Status</th></tr></thead>
			<tbody class="divide-y divide-slate-100">
				{#each rows as r (r.id)}
					<tr class="cursor-pointer hover:bg-slate-50" onclick={() => goto(`/accounting/journal/${r.id}`)} data-entry={r.number ?? 'draft'}>
						<td class="px-4 py-2 text-slate-500">{r.number ?? '—'}</td>
						<td class="px-4 py-2 whitespace-nowrap">{formatDate(r.date)}</td>
						<td class="px-4 py-2">{r.memo ?? ''}</td>
						<td class="px-4 py-2">{SOURCES[r.source] ?? r.source}{r.reverses ? ' (reversal)' : ''}</td>
						<td class="px-4 py-2 text-right whitespace-nowrap">{amount(r.amount, cur)}</td>
						<td class="px-4 py-2"><span class="badge {STATUS_BADGE[r.status]}">{r.status === 'void' ? 'reversed' : r.status}</span></td>
					</tr>
				{:else}
					<tr><td colspan="6" class="px-4 py-10 text-center text-slate-500">No entries.</td></tr>
				{/each}
			</tbody>
		</table>
		{#if more}<div class="p-3 text-center"><button class="text-sm text-brand-600 hover:underline" onclick={() => load(true)}>More</button></div>{/if}
	</div>
</div>
