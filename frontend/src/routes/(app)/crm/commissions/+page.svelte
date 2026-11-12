<script lang="ts">
	// Commissions: my month, my team's (or everyone's), and — for those who manage them — the
	// plans, locking a month and the payroll export.
	import { onMount } from 'svelte';
	import { api, downloadUrl } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';

	type Entry = { id: string; plan: string; invoice_id: string; invoice_number: number; invoice_title: string; base: string; rate: string; amount: string; reason: string; at: string; link: string };
	type Statement = { user: { id: string; name: string }; month: string; currency: string; locked_at: string | null; earned: string; taken_back: string; net: string; entries: Entry[] };
	type Person = { id: string; name: string; email: string; earned: string; taken_back: string; net: string };
	type Summary = { month: string; locked_at: string | null; net: string; people: Person[]; can_manage: boolean };
	type Plan = { id: string; name: string; is_active: boolean; rate: string; tiers: { from: string; rate: string }[]; product_ids: string[]; user_ids: string[]; department_ids: string[] };

	const canManage = session.can(PERM.COMMISSIONS_MANAGE);
	let month = $state(new Date().toISOString().slice(0, 7));
	let who = $state<string | null>(null);
	let st = $state<Statement | null>(null);
	let summary = $state<Summary | null>(null);
	let plans = $state<Plan[]>([]);
	let tab = $state<'mine' | 'team' | 'plans'>('mine');
	let draft = $state<{ id?: string; name: string; rate: string; tiers: { from: string; rate: string }[]; people: PickedUser[]; is_active: boolean } | null>(null);

	const money = (v: string | number, cur = st?.currency ?? '') => `${Number(v).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })} ${cur}`;

	async function loadStatement() {
		try {
			st = await api.get<Statement>(`/commissions/statement?month=${month}${who ? `&user=${who}` : ''}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function loadSummary() {
		summary = await api.get<Summary>(`/commissions/summary?month=${month}`).catch(() => null);
	}
	async function loadPlans() {
		if (canManage) plans = (await api.get<{ items: Plan[] }>('/commissions/plans')).items;
	}
	onMount(async () => {
		await Promise.all([loadStatement(), loadSummary(), loadPlans()]);
	});
	function changeMonth() {
		loadStatement();
		loadSummary();
	}
	function open(p: Person) {
		who = p.id;
		tab = 'mine';
		loadStatement();
	}
	async function toggleLock() {
		try {
			if (summary?.locked_at) await api.del(`/commissions/months/${month}/lock`);
			else await api.post(`/commissions/months/${month}/lock`, {});
			await loadSummary();
			await loadStatement();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function recalc() {
		try {
			const r = await api.post<{ invoices: number; new_entries: number }>('/commissions/recalculate', {});
			toasts.success(r.new_entries ? `${r.new_entries} changes from ${r.invoices} invoices` : 'Everything was already up to date');
			changeMonth();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function savePlan(e: SubmitEvent) {
		e.preventDefault();
		if (!draft) return;
		// Amounts go as text (the server reads decimals exactly).
		const body = { name: draft.name, rate: String(draft.rate), is_active: draft.is_active, tiers: draft.tiers.filter((t) => t.from && t.rate).map((t) => ({ from: String(t.from), rate: String(t.rate) })), user_ids: draft.people.map((p) => p.id) };
		try {
			if (draft.id) await api.patch(`/commissions/plans/${draft.id}`, body);
			else await api.post('/commissions/plans', body);
			draft = null;
			await loadPlans();
			toasts.success('Plan saved');
		} catch (err) {
			toasts.error(err);
		}
	}
	async function editPlan(p: Plan) {
		const people: PickedUser[] = [];
		for (const id of p.user_ids) {
			const u = await api.get<{ id: string; first_name: string; last_name: string; avatar_file_id: string | null }>(`/users/${id}`).catch(() => null);
			if (u) people.push({ id: u.id, name: `${u.first_name} ${u.last_name}`, avatar_file_id: u.avatar_file_id });
		}
		draft = { id: p.id, name: p.name, rate: p.rate, tiers: p.tiers.map((t) => ({ from: String(t.from), rate: String(t.rate) })), people, is_active: p.is_active };
	}
</script>

<svelte:head><title>Commissions · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Commissions</h1>
		<input type="month" class="input w-44" bind:value={month} onchange={changeMonth} aria-label="Month" />
		<div class="ml-auto flex gap-1" role="tablist">
			<button role="tab" class="rounded-full px-3 py-1.5 text-sm {tab === 'mine' ? 'bg-slate-900 text-white' : 'bg-white ring-1 ring-slate-200'}" onclick={() => ((tab = 'mine'), (who = null), loadStatement())} data-tab="mine">Mine</button>
			{#if summary}<button role="tab" class="rounded-full px-3 py-1.5 text-sm {tab === 'team' ? 'bg-slate-900 text-white' : 'bg-white ring-1 ring-slate-200'}" onclick={() => (tab = 'team')} data-tab="team">{canManage ? 'Everyone' : 'My team'}</button>{/if}
			{#if canManage}<button role="tab" class="rounded-full px-3 py-1.5 text-sm {tab === 'plans' ? 'bg-slate-900 text-white' : 'bg-white ring-1 ring-slate-200'}" onclick={() => (tab = 'plans')} data-tab="plans">Plans</button>{/if}
		</div>
	</div>

	{#if tab === 'mine' && st}
		<section class="card p-5 text-sm" aria-label="Statement">
			<div class="flex flex-wrap items-baseline gap-x-6 gap-y-1">
				<h2 class="font-semibold text-slate-900">{st.user.name}{who ? '' : ' (you)'} · {st.month}</h2>
				<span>Earned <b data-earned>{money(st.earned)}</b></span>
				<span>Taken back <b class="text-red-700" data-taken-back>{money(st.taken_back)}</b></span>
				<span>Net <b data-net>{money(st.net)}</b></span>
				{#if st.locked_at}<span class="text-xs text-slate-500">locked {formatDateTime(st.locked_at)}</span>{/if}
			</div>
			<p class="mt-1 text-xs text-slate-500">Earned as invoices are paid (on the sale without tax); taken back when a payment is voided or a paid invoice is credited.</p>
			<table class="mt-3 min-w-full text-sm">
				<thead class="text-left text-xs text-slate-500"><tr><th class="py-1">Invoice</th><th>Plan</th><th>What happened</th><th class="text-right">On</th><th class="text-right">Rate</th><th class="text-right">Amount</th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each st.entries as e (e.id)}
						<tr data-entry={e.invoice_number}>
							<td class="py-1.5"><a class="text-brand-700 hover:underline" href={e.link}>#{e.invoice_number}</a> <span class="text-slate-500">{e.invoice_title}</span></td>
							<td>{e.plan}</td>
							<td class="text-slate-600">{e.reason}</td>
							<td class="text-right tabular-nums">{money(e.base, '')}</td>
							<td class="text-right tabular-nums">{Number(e.rate)}%</td>
							<td class="text-right tabular-nums {Number(e.amount) < 0 ? 'text-red-700' : ''}">{money(e.amount, '')}</td>
						</tr>
					{:else}<tr><td colspan="6" class="py-3 text-slate-500">Nothing this month.</td></tr>{/each}
				</tbody>
			</table>
		</section>
	{:else if tab === 'team' && summary}
		<section class="card p-5 text-sm" aria-label="Team">
			<div class="flex flex-wrap items-center gap-3">
				<h2 class="font-semibold text-slate-900">{summary.month}: {money(summary.net)}</h2>
				{#if summary.locked_at}<span class="badge bg-slate-100 text-slate-700" data-locked>Locked {formatDateTime(summary.locked_at)}</span>{/if}
				{#if canManage}
					<div class="ml-auto flex gap-2">
						<button class="btn btn-secondary" onclick={recalc}>Recalculate</button>
						<button class="btn btn-secondary" onclick={() => downloadUrl(`/api/commissions/export.csv?month=${month}`, `commissions-${month}.csv`)}>Payroll CSV</button>
						<button class="btn btn-primary" onclick={toggleLock} data-lock>{summary.locked_at ? 'Unlock' : 'Lock for payroll'}</button>
					</div>
				{/if}
			</div>
			<table class="mt-3 min-w-full text-sm">
				<thead class="text-left text-xs text-slate-500"><tr><th class="py-1">Person</th><th class="text-right">Earned</th><th class="text-right">Taken back</th><th class="text-right">Net</th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each summary.people as p (p.id)}
						<tr data-person={p.name}><td class="py-1.5"><button class="text-brand-700 hover:underline" onclick={() => open(p)}>{p.name}</button></td><td class="text-right">{money(p.earned, '')}</td><td class="text-right text-red-700">{money(p.taken_back, '')}</td><td class="text-right font-medium">{money(p.net, '')}</td></tr>
					{:else}<tr><td colspan="4" class="py-3 text-slate-500">Nothing this month.</td></tr>{/each}
				</tbody>
			</table>
		</section>
	{:else if tab === 'plans'}
		<section class="card divide-y divide-slate-100 text-sm" aria-label="Plans">
			{#each plans as p (p.id)}
				<div class="flex items-center gap-3 px-5 py-3" data-plan={p.name}>
					<div class="flex-1">
						<p class="font-medium {p.is_active ? 'text-slate-900' : 'text-slate-400'}">{p.name}{p.is_active ? '' : ' (off)'}</p>
						<p class="text-xs text-slate-500">{Number(p.rate)}% of the paid sale{p.tiers.length ? `, ${p.tiers.map((t) => `${Number(t.rate)}% past ${Number(t.from).toLocaleString()} a month`).join(', ')}` : ''}{p.product_ids.length ? ` · ${p.product_ids.length} products` : ''} · {p.user_ids.length || p.department_ids.length ? `${p.user_ids.length} people` : 'everyone'}</p>
					</div>
					<button class="text-brand-700 hover:underline" onclick={() => editPlan(p)}>Edit</button>
				</div>
			{:else}<p class="px-5 py-4 text-slate-500">No plans yet.</p>{/each}
			<div class="px-5 py-3"><button class="btn btn-secondary" onclick={() => (draft = { name: '', rate: '5', tiers: [], people: [], is_active: true })} data-new-plan>New plan</button></div>
		</section>
		{#if draft}
			<form class="card grid gap-3 p-5 text-sm sm:grid-cols-2" onsubmit={savePlan} aria-label="Plan">
				<label class="block">Name<input class="input mt-1" required bind:value={draft.name} name="name" /></label>
				<label class="block">Percent of the paid sale (without tax)<input class="input mt-1" type="number" min="0" max="100" step="0.01" required bind:value={draft.rate} name="rate" /></label>
				<div class="sm:col-span-2">
					<p class="mb-1">Higher rates once a person's paid sales in the month pass…</p>
					{#each draft.tiers as t, i (i)}
						<div class="mb-1 flex items-center gap-2"><input class="input w-36" type="number" min="0" placeholder="amount" bind:value={t.from} aria-label="From" /> → <input class="input w-24" type="number" min="0" max="100" step="0.01" placeholder="%" bind:value={t.rate} aria-label="Rate" /> % <button type="button" class="text-slate-400" onclick={() => draft!.tiers.splice(i, 1)}>×</button></div>
					{/each}
					<button type="button" class="text-brand-700 hover:underline" onclick={() => draft!.tiers.push({ from: '', rate: '' })}>+ Tier</button>
				</div>
				<div class="sm:col-span-2"><p class="mb-1">Who earns under it (nobody chosen: everyone responsible for invoices)</p><MultiUserPicker bind:value={draft.people} /></div>
				<label class="flex items-center gap-2"><input type="checkbox" bind:checked={draft.is_active} /> On</label>
				<div class="flex gap-2 sm:col-span-2"><button class="btn btn-primary">Save plan</button><button type="button" class="btn btn-ghost" onclick={() => (draft = null)}>Cancel</button></div>
			</form>
		{/if}
	{/if}
</div>
