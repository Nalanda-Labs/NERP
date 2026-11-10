<script lang="ts">
	// On a CRM invoice: whether it is in the books, what has been paid and what is left.
	import { api } from '#lib/api.ts';
	import { amount, STATUS_BADGE, type Party } from '#lib/accounting.ts';
	import PaymentModal from '#lib/components/accounting/PaymentModal.svelte';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	let { itemId, onchanged }: { itemId: string; onchanged?: () => void } = $props();
	type Info = { status: string; entry_id?: string; entry_number?: number; date?: string; total?: string; paid?: string; balance?: string; currency?: string; party_id?: string; party_name?: string; payments: { id: string; number: number; date: string; amount: string; status: string }[] };
	let info = $state<Info | null>(null);
	const canPost = session.can(PERM.ACCOUNTING_POST) || session.can(PERM.ACCOUNTING_ADMIN);
	const load = async () => (info = await api.get<Info>(`/accounting/invoices/${itemId}`).catch(() => null));
	$effect(() => {
		itemId;
		load();
	});
	async function post() {
		try {
			await api.post(`/accounting/invoices/${itemId}/post`, {});
			toasts.success('Posted to the books');
			await load();
			onchanged?.();
		} catch (e) {
			toasts.error(e);
		}
	}
	let payOpen = $state(false);
	const party = $derived<Party | null>(info?.party_id ? { id: info.party_id, name: info.party_name ?? '', kind: 'company' } : null);
</script>

{#if info}
	<section class="card p-4 text-sm" data-books>
		<div class="mb-2 flex items-center justify-between">
			<h3 class="font-semibold text-slate-900">In the books</h3>
			<span class="badge {STATUS_BADGE[info.status]} capitalize">{info.status === 'unposted' ? 'Not posted' : info.status}</span>
		</div>
		{#if info.status === 'unposted' || info.status === 'void'}
			<p class="text-slate-500">{info.status === 'void' ? 'Voided in the books; it can be posted again.' : 'Not in the accounts yet.'}</p>
			{#if canPost}<button class="btn btn-secondary mt-2 w-full justify-center" onclick={post}>Post to the books</button>{/if}
		{:else}
			<dl class="grid grid-cols-2 gap-y-1">
				<dt class="text-slate-500">Posted</dt><dd class="text-right">{formatDate(info.date)}{#if info.entry_id}<a class="ml-1 text-brand-700 hover:underline" href="/accounting/journal/{info.entry_id}">#{info.entry_number}</a>{/if}</dd>
				<dt class="text-slate-500">Paid</dt><dd class="text-right">{amount(info.paid, info.currency ?? '')}</dd>
				<dt class="font-medium">Left to pay</dt><dd class="text-right font-medium" data-books-balance>{amount(info.balance, info.currency ?? '')}</dd>
			</dl>
			{#if info.payments.length}<ul class="mt-2 space-y-0.5 text-xs text-slate-500">{#each info.payments as p (p.id)}<li class={p.status === 'void' ? 'line-through' : ''}>Payment {p.number} · {formatDate(p.date)} · {amount(p.amount, info.currency ?? '')}</li>{/each}</ul>{/if}
			{#if canPost && info.status === 'open'}<button class="btn btn-primary mt-2 w-full justify-center" onclick={() => (payOpen = true)}>Receive payment</button>{/if}
		{/if}
	</section>
	<PaymentModal bind:open={payOpen} direction="in" {party} docId={itemId} onsaved={() => (load(), onchanged?.())} />
{/if}
