<script lang="ts">
	// The chart of accounts.
	import { api } from '#lib/api.ts';
	import { accounts, amount, baseCurrency, natural, subtypeLabel, SUBTYPES, TYPES, type Account } from '#lib/accounting.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	let list = $state<Account[]>([]);
	let showInactive = $state(false);
	let cur = $state('');
	const admin = session.can(PERM.ACCOUNTING_ADMIN);
	const load = async () => (list = await accounts(true));
	$effect(() => {
		load();
		baseCurrency().then((c) => (cur = c));
	});

	let edit = $state<Partial<Account> & { id?: string } | null>(null);
	function open(a: Account | null) {
		edit = a ? { ...a } : { code: '', name: '', subtype: 'expense', currency: null, description: '', is_active: true };
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		const body = { code: edit!.code, name: edit!.name, subtype: edit!.subtype, currency: edit!.currency || null, description: edit!.description || null, is_active: edit!.is_active };
		try {
			if (edit!.id) await api.patch(`/accounting/accounts/${edit!.id}`, edit!.used ? { code: body.code, name: body.name, description: body.description, is_active: body.is_active } : body);
			else await api.post('/accounting/accounts', body);
			edit = null;
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove() {
		if (!confirm(`Delete ${edit!.code} · ${edit!.name}?`)) return;
		try {
			await api.del(`/accounting/accounts/${edit!.id}`);
			edit = null;
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>Chart of accounts · Accounting · NERP</title></svelte:head>

<div class="max-w-4xl space-y-5">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Chart of accounts</h1>
			<p class="text-sm text-slate-500">Balances as accountants read them: assets and expenses as debits, the rest as credits.</p>
		</div>
		<div class="flex items-center gap-3">
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={showInactive} /> Inactive too</label>
			{#if admin}<button class="btn btn-primary" onclick={() => open(null)}><Icon name="plus" class="size-4" /> Account</button>{/if}
		</div>
	</div>
	{#each TYPES as [type, title] (type)}
		{@const items = list.filter((a) => a.type === type && (showInactive || a.is_active))}
		{#if items.length}
			<section class="card">
				<h2 class="border-b border-slate-100 px-4 py-2 font-semibold text-slate-900">{title}</h2>
				<ul class="divide-y divide-slate-100 text-sm">
					{#each items as a (a.id)}
						<li class="flex items-center gap-3 px-4 py-2 {a.is_active ? '' : 'text-slate-400'}" data-account={a.code}>
							<span class="w-28 font-mono text-xs">{a.code}</span>
							<a class="min-w-0 flex-1 truncate hover:text-brand-700" href="/accounting/reports?tab=ledger&account={a.id}">{a.name}{a.currency ? ` (${a.currency})` : ''}</a>
							<span class="hidden w-40 text-xs text-slate-500 sm:block">{subtypeLabel(a.subtype)}</span>
							<span class="w-36 text-right">{amount(natural(a.balance, a.type), cur)}</span>
							{#if admin}<button class="text-slate-400 hover:text-slate-700" aria-label="Edit {a.code}" onclick={() => open(a)}><Icon name="edit" class="size-4" /></button>{/if}
						</li>
					{/each}
				</ul>
			</section>
		{/if}
	{/each}
</div>

<Modal open={!!edit} title={edit?.id ? 'Edit account' : 'New account'} onclose={() => (edit = null)}>
	{#if edit}
		<form id="account-form" class="grid gap-3 sm:grid-cols-3" onsubmit={save}>
			<div><label class="label" for="a-code">Code</label><input id="a-code" class="input" required maxlength="20" bind:value={edit.code} /></div>
			<div class="sm:col-span-2"><label class="label" for="a-name">Name</label><input id="a-name" class="input" required bind:value={edit.name} /></div>
			<div class="sm:col-span-2">
				<label class="label" for="a-kind">Kind</label>
				<select id="a-kind" class="input" disabled={!!edit.used} bind:value={edit.subtype}>{#each TYPES as [t, tt] (t)}<optgroup label={tt}>{#each SUBTYPES.filter((s) => s[1] === t) as [s, , label] (s)}<option value={s}>{label}</option>{/each}</optgroup>{/each}</select>
			</div>
			{#if edit.subtype === 'bank' || edit.subtype === 'cash'}<div><label class="label" for="a-cur">Currency</label><input id="a-cur" class="input uppercase" maxlength="3" placeholder={cur} disabled={!!edit.used} bind:value={edit.currency} /></div>{/if}
			<div class="sm:col-span-3"><label class="label" for="a-desc">Description</label><input id="a-desc" class="input" bind:value={edit.description} /></div>
			{#if edit.id}<label class="flex items-center gap-2 text-sm sm:col-span-3"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={edit.is_active} /> Active</label>{/if}
			{#if edit.used}<p class="text-xs text-slate-500 sm:col-span-3">This account has entries: its kind and currency stay as they are.</p>{/if}
		</form>
	{/if}
	{#snippet footer()}
		{#if edit?.id && !edit.used}<button class="btn btn-ghost mr-auto text-red-600" onclick={remove}>Delete</button>{/if}
		<button class="btn btn-secondary" onclick={() => (edit = null)}>Cancel</button>
		<button class="btn btn-primary" form="account-form">Save</button>
	{/snippet}
</Modal>
