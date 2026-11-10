<script lang="ts">
	// The CRM's invoices in the books: posting them, payments received, voiding.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, query } from '#lib/api.ts';
	import { amount, STATUS_BADGE, today, type Party } from '#lib/accounting.ts';
	import PaymentModal from '#lib/components/accounting/PaymentModal.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Row = { item_id: string; number: number; title: string; currency: string; party_id: string | null; party_name: string | null; stage: string | null; amount: string; date: string | null; due_date: string | null; total: string | null; paid: string; balance: string | null; status: string; overdue_days: number | null };
	const TABS = [['unposted', 'To post'], ['open', 'Open'], ['overdue', 'Overdue'], ['paid', 'Paid'], ['void', 'Void'], ['all', 'All']] as const;
	const status = $derived(page.url.searchParams.get('status') ?? 'open');
	let rows = $state<Row[] | null>(null);
	let q = $state('');
	const canPost = session.can(PERM.ACCOUNTING_POST) || session.can(PERM.ACCOUNTING_ADMIN);

	async function load() {
		rows = (await api.get<{ items: Row[] }>('/accounting/invoices' + query({ status, q: q || undefined, limit: 500 }))).items;
	}
	$effect(() => {
		status;
		load();
	});
	let t: ReturnType<typeof setTimeout> | undefined;
	const search = () => (clearTimeout(t), (t = setTimeout(load, 250)));

	// Posting
	let postFor = $state<Row | null>(null);
	let postDate = $state(today());
	let fx = $state('');
	let busy = $state(false);
	async function post(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			await api.post(`/accounting/invoices/${postFor!.item_id}/post`, { date: postDate, fx_rate: fx || null });
			toasts.success(`Invoice ${postFor!.number} is in the books`);
			postFor = null;
			load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function voidIt(r: Row) {
		if (!confirm(`Void invoice ${r.number} in the books? A reversing entry is posted and the invoice can be changed in the CRM again.`)) return;
		try {
			await api.post(`/accounting/invoices/${r.item_id}/void`, {});
			toasts.success('Voided');
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
	// Payment
	let payOpen = $state(false);
	let payParty = $state<Party | null>(null);
	let payDoc = $state<string | null>(null);
	function receive(r: Row) {
		payParty = r.party_id ? { id: r.party_id, name: r.party_name ?? '', kind: 'company' } : null;
		payDoc = r.item_id;
		payOpen = true;
	}
</script>

<svelte:head><title>Sales invoices · Accounting · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Sales invoices</h1>
			<p class="text-sm text-slate-500">Invoices are made in the CRM; here they go into the books and get paid.</p>
		</div>
		<input class="input w-64" type="search" placeholder="Number, title or customer" aria-label="Search invoices" bind:value={q} oninput={search} />
	</div>
	<div class="flex flex-wrap gap-1 border-b border-slate-200" role="tablist">
		{#each TABS as [k, label] (k)}
			<button role="tab" aria-selected={status === k} class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {status === k ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500 hover:text-slate-800'}" onclick={() => goto(`?status=${k}`, { replaceState: true })}>{label}</button>
		{/each}
	</div>

	{#if rows}
		<div class="card overflow-x-auto">
			<table class="w-full text-sm">
				<thead class="border-b border-slate-100 text-left text-xs text-slate-500 uppercase">
					<tr><th class="px-4 py-2">No.</th><th class="px-4 py-2">Invoice</th><th class="px-4 py-2">Customer</th><th class="px-4 py-2">Date</th><th class="px-4 py-2">Due</th><th class="px-4 py-2 text-right">Total</th><th class="px-4 py-2 text-right">Left to pay</th><th class="px-4 py-2">Status</th><th class="px-4 py-2"></th></tr>
				</thead>
				<tbody class="divide-y divide-slate-100">
					{#each rows as r (r.item_id)}
						<tr data-invoice={r.number}>
							<td class="px-4 py-2 text-slate-500">{r.number}</td>
							<td class="px-4 py-2"><a class="font-medium text-slate-800 hover:text-brand-700" href="/crm/invoice/{r.item_id}">{r.title}</a>{#if r.stage}<span class="ml-1 text-xs text-slate-400">· {r.stage}</span>{/if}</td>
							<td class="px-4 py-2">{r.party_name ?? '—'}</td>
							<td class="px-4 py-2 whitespace-nowrap">{formatDate(r.date)}</td>
							<td class="px-4 py-2 whitespace-nowrap {r.overdue_days ? 'font-medium text-red-600' : ''}">{formatDate(r.due_date)}{#if r.overdue_days}<span class="block text-xs">{r.overdue_days} days late</span>{/if}</td>
							<td class="px-4 py-2 text-right whitespace-nowrap">{amount(r.total ?? r.amount, r.currency)}</td>
							<td class="px-4 py-2 text-right whitespace-nowrap">{r.balance !== null && r.status !== 'void' ? amount(r.balance, r.currency) : ''}</td>
							<td class="px-4 py-2"><span class="badge {STATUS_BADGE[r.status]} capitalize">{r.status === 'unposted' ? 'Not posted' : r.status}</span></td>
							<td class="px-4 py-2 text-right whitespace-nowrap">
								{#if canPost}
									{#if r.status === 'unposted' || r.status === 'void'}<button class="btn btn-secondary py-1 text-xs" onclick={() => ((postFor = r), (postDate = today()), (fx = ''))}>Post</button>{/if}
									{#if r.status === 'open'}<button class="btn btn-primary py-1 text-xs" onclick={() => receive(r)}>Receive payment</button>{/if}
									{#if (r.status === 'open' && Number(r.paid) === 0)}<button class="btn btn-ghost py-1 text-xs text-red-600" onclick={() => voidIt(r)}>Void</button>{/if}
								{/if}
							</td>
						</tr>
					{:else}
						<tr><td colspan="9" class="px-4 py-10 text-center text-slate-500">No invoices here.</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<Modal open={!!postFor} title="Post invoice {postFor?.number ?? ''}" onclose={() => (postFor = null)}>
	{#if postFor}
		<form id="post-invoice" class="space-y-3" onsubmit={post}>
			<p class="text-sm text-slate-600">{postFor.title} · {postFor.party_name ?? 'no client'} · {amount(postFor.amount, postFor.currency)}</p>
			<div><label class="label" for="post-date">Accounting date</label><input id="post-date" type="date" class="input" required bind:value={postDate} /></div>
			<div><label class="label" for="post-fx">Exchange rate <span class="font-normal text-slate-400">(other currencies; empty: the CRM's)</span></label><input id="post-fx" class="input" inputmode="decimal" bind:value={fx} /></div>
			<p class="text-xs text-slate-500">After posting, the invoice's amount, products, currency and client are locked in the CRM until it is voided here.</p>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (postFor = null)}>Cancel</button>
		<button class="btn btn-primary" form="post-invoice" disabled={busy}>Post to the books</button>
	{/snippet}
</Modal>

<PaymentModal bind:open={payOpen} direction="in" party={payParty} docId={payDoc} onsaved={load} />
