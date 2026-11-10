<script lang="ts">
	// Help desk: the ticket lists (support sees every ticket, employees their own requests)
	// and asking for help.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { formatRelative } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { CHANNEL_LABEL, dueIn, loadDictionary, nameOf, type Entry, type Ticket } from '#lib/support.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM, type Page } from '#lib/types.ts';

	const support = $derived(session.can(PERM.SUPPORT_WORK) || session.can(PERM.SUPPORT_MANAGE));
	const manager = $derived(session.can(PERM.SUPPORT_MANAGE));
	const SCOPES = [
		{ id: 'open', label: 'Open' },
		{ id: 'assigned', label: 'Assigned to me' },
		{ id: 'unassigned', label: 'Unassigned' },
		{ id: 'overdue', label: 'Overdue' },
		{ id: 'closed', label: 'Closed' },
		{ id: 'mine', label: 'My requests' },
		{ id: 'spam', label: 'Spam' }
	];
	let scope = $state('mine');
	let dict = $state<Entry[]>([]);
	let tickets = $state<Ticket[]>([]);
	let total = $state(0);
	let loaded = $state(false);
	let q = $state('');
	let categoryId = $state('');
	let statusId = $state('');
	let now = $state(Date.now());
	const of = (kind: string) => dict.filter((e) => e.kind === kind);

	async function load() {
		try {
			const r = await api.get<Page<Ticket>>(`/support/tickets${query({ scope, q, category_id: categoryId, status_id: statusId, limit: 100 })}`);
			tickets = r.items;
			total = r.total;
			now = Date.now();
		} catch (e) {
			toasts.error(e);
		} finally {
			loaded = true;
		}
	}
	onMount(async () => {
		if (support) scope = 'open';
		try {
			dict = await loadDictionary();
		} catch (e) {
			toasts.error(e);
		}
		await load();
	});
	let searchTimer: ReturnType<typeof setTimeout>;
	function search() {
		clearTimeout(searchTimer);
		searchTimer = setTimeout(load, 250);
	}

	// ---- asking for help ----
	let open = $state(false);
	let saving = $state(false);
	let form = $state({ title: '', body: '', category_id: '', criticality_id: '', for: 'me' as 'me' | 'colleague' | 'customer', requester_id: null as string | null, customer_email: '', customer_name: '' });
	let files = $state<{ id: string; name: string }[]>([]);
	let fileInput = $state<HTMLInputElement>();
	function start() {
		form = { title: '', body: '', category_id: of('category').find((e) => e.is_default)?.id ?? '', criticality_id: of('criticality').find((e) => e.is_default)?.id ?? '', for: 'me', requester_id: null, customer_email: '', customer_name: '' };
		files = [];
		open = true;
	}
	async function attach(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		for (const f of Array.from(input.files ?? [])) {
			const fd = new FormData();
			fd.append('file', f);
			try {
				const meta = await api.post<{ id: string; original_name: string }>('/files', fd);
				files = [...files, { id: meta.id, name: meta.original_name ?? f.name }];
			} catch (err) {
				toasts.error(err);
			}
		}
		input.value = '';
	}
	async function send(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			const t = await api.post<Ticket>('/support/tickets', {
				title: form.title,
				body: form.body,
				category_id: form.category_id || null,
				criticality_id: form.criticality_id || null,
				file_ids: files.map((f) => f.id),
				requester_id: form.for === 'colleague' ? form.requester_id : null,
				customer_email: form.for === 'customer' ? form.customer_email : null,
				customer_name: form.for === 'customer' ? form.customer_name || null : null
			});
			open = false;
			toasts.success(`Request #${t.number} sent`);
			await goto(`/support/tickets/${t.id}`);
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
</script>

<svelte:head><title>Help desk · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Help desk</h1>
		<div class="flex flex-wrap gap-2">
			{#if support}<a class="btn btn-secondary" href="/support/stats"><Icon name="columns" class="size-4" />Statistics</a>{/if}
			{#if manager}<a class="btn btn-secondary" href="/support/settings"><Icon name="settings" class="size-4" />Settings</a>{/if}
			<button class="btn btn-primary" onclick={start}><Icon name="plus" class="size-4" />New request</button>
		</div>
	</div>

	{#if support}
		<div class="flex flex-wrap gap-1 border-b border-slate-200" role="tablist">
			{#each SCOPES as s (s.id)}
				<button role="tab" aria-selected={scope === s.id} class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {scope === s.id ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500 hover:text-slate-800'}" onclick={() => ((scope = s.id), load())}>{s.label}</button>
			{/each}
		</div>
		<div class="flex flex-wrap gap-2">
			<input class="input max-w-xs" type="search" placeholder="Search: title, #number, email" aria-label="Search tickets" bind:value={q} oninput={search} />
			<select class="input w-auto" aria-label="Category" bind:value={categoryId} onchange={load}>
				<option value="">All categories</option>
				{#each of('category') as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
			</select>
			<select class="input w-auto" aria-label="Status" bind:value={statusId} onchange={load}>
				<option value="">All statuses</option>
				{#each of('status') as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
			</select>
		</div>
	{:else}
		<p class="text-slate-600">Ask the support team for help. Your requests and their answers are here.</p>
	{/if}

	<div class="card overflow-x-auto">
		<table class="w-full text-sm">
			<thead class="bg-slate-50 text-left text-xs font-medium tracking-wide text-slate-500 uppercase">
				<tr>
					<th class="px-4 py-2.5">#</th>
					<th class="px-4 py-2.5">Request</th>
					{#if support}<th class="px-4 py-2.5">From</th>{/if}
					<th class="px-4 py-2.5">Status</th>
					{#if support}<th class="px-4 py-2.5">Responsible</th><th class="px-4 py-2.5">Reply</th>{/if}
					<th class="px-4 py-2.5">Last message</th>
				</tr>
			</thead>
			<tbody class="divide-y divide-slate-100">
				{#each tickets as t (t.id)}
					{@const due = t.closed_at ? null : dueIn(t.deadline_at, now)}
					<tr class="hover:bg-slate-50" data-ticket={t.number}>
						<td class="px-4 py-2.5 text-slate-500">{t.number}</td>
						<td class="px-4 py-2.5">
							<a class="font-medium text-slate-900 hover:underline" href="/support/tickets/{t.id}">{t.title}</a>
							<div class="text-xs text-slate-500">{nameOf(dict, t.category_id)}{#if nameOf(dict, t.criticality_id)}{' · '}{nameOf(dict, t.criticality_id)}{/if}</div>
						</td>
						{#if support}<td class="px-4 py-2.5 text-slate-600">{t.requester_name ?? t.requester_email}<div class="text-xs text-slate-400">{CHANNEL_LABEL[t.channel]}</div></td>{/if}
						<td class="px-4 py-2.5">
							{#if t.closed_at}<span class="rounded-full bg-slate-100 px-2 py-0.5 text-xs text-slate-600">Closed</span>
							{:else if t.on_hold}<span class="rounded-full bg-amber-50 px-2 py-0.5 text-xs text-amber-700">On hold</span>
							{:else if t.waiting_for === 'customer'}<span class="rounded-full bg-sky-50 px-2 py-0.5 text-xs text-sky-700">{support ? 'Waiting for customer' : 'Answered'}</span>
							{:else}<span class="rounded-full bg-emerald-50 px-2 py-0.5 text-xs text-emerald-700">{nameOf(dict, t.status_id) || 'Open'}</span>{/if}
						</td>
						{#if support}
							<td class="px-4 py-2.5 text-slate-600">{t.responsible_name ?? '—'}</td>
							<td class="px-4 py-2.5 whitespace-nowrap">{#if due}<span class={due.late || t.is_overdue ? 'font-medium text-red-600' : 'text-slate-600'} data-due>{due.text}</span>{/if}</td>
						{/if}
						<td class="px-4 py-2.5 whitespace-nowrap text-slate-500">{formatRelative(t.last_message_at)}</td>
					</tr>
				{:else}
					{#if loaded}<tr><td colspan="7" class="px-4 py-10 text-center text-slate-500">{support ? 'No tickets here.' : 'You have not asked for help yet.'}</td></tr>{/if}
				{/each}
			</tbody>
		</table>
	</div>
	{#if total > tickets.length}<p class="text-sm text-slate-500">Showing {tickets.length} of {total}; search to narrow it down.</p>{/if}
</div>

<Modal bind:open title="New request" size="lg">
	<form id="new-ticket" class="space-y-4" onsubmit={send}>
		{#if support}
			<fieldset class="flex flex-wrap gap-4 text-sm">
				<legend class="label">For</legend>
				<label class="flex items-center gap-2"><input type="radio" bind:group={form.for} value="me" />Me</label>
				<label class="flex items-center gap-2"><input type="radio" bind:group={form.for} value="colleague" />A colleague</label>
				<label class="flex items-center gap-2"><input type="radio" bind:group={form.for} value="customer" />A customer</label>
			</fieldset>
			{#if form.for === 'colleague'}<div><span class="label">Colleague</span><UserPicker bind:value={form.requester_id} /></div>{/if}
			{#if form.for === 'customer'}
				<div class="grid gap-3 sm:grid-cols-2">
					<div><label class="label" for="t-cemail">Customer's email</label><input id="t-cemail" class="input" type="email" required bind:value={form.customer_email} /></div>
					<div><label class="label" for="t-cname">Name</label><input id="t-cname" class="input" bind:value={form.customer_name} /></div>
				</div>
			{/if}
		{/if}
		<div><label class="label" for="t-title">Subject</label><input id="t-title" class="input" required maxlength="300" bind:value={form.title} /></div>
		<div class="grid gap-3 sm:grid-cols-2">
			<div>
				<label class="label" for="t-cat">Category</label>
				<select id="t-cat" class="input" bind:value={form.category_id}>{#each of('category') as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</select>
			</div>
			<div>
				<label class="label" for="t-crit">Criticality</label>
				<select id="t-crit" class="input" bind:value={form.criticality_id}>{#each of('criticality') as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</select>
			</div>
		</div>
		<div><label class="label" for="t-body">What happened?</label><textarea id="t-body" class="input min-h-36" required bind:value={form.body}></textarea></div>
		<div class="flex flex-wrap items-center gap-2">
			<button type="button" class="btn btn-secondary" onclick={() => fileInput?.click()}><Icon name="paperclip" class="size-4" />Attach files</button>
			<input bind:this={fileInput} class="hidden" type="file" multiple aria-label="Attach files" onchange={attach} />
			{#each files as f (f.id)}<span class="rounded-full bg-slate-100 px-2.5 py-1 text-xs text-slate-700">{f.name} <button type="button" class="ml-1 text-slate-400 hover:text-red-600" aria-label="Remove {f.name}" onclick={() => (files = files.filter((x) => x.id !== f.id))}>×</button></span>{/each}
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="new-ticket" type="submit" disabled={saving}>Send</button>
	{/snippet}
</Modal>
