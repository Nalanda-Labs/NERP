<script lang="ts">
	// Duplicate search over a whole record type: groups of records sharing a phone, an email or
	// a name. Merge the groups chosen (each into the record to keep, the oldest by default), or
	// mark a group "not duplicates" so it stops showing.
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { crm, type CrmType } from '#lib/crm.svelte.ts';
	import { formatDate } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Item = { id: string; number: number; title: string; assigned_name: string | null; created_at: string; phones: string | null; emails: string | null; deals: number };
	type Group = { kind: 'phone' | 'email' | 'name'; value: string; shown: string; count: number; items: Item[] };

	const code = $derived(page.params.type!);
	let type = $state<CrmType | null>(null);
	let by = $state({ phone: true, email: true, name: true });
	let groups = $state<Group[]>([]);
	let total = $state(0);
	let search = $state('');
	let timer: ReturnType<typeof setTimeout>;
	let offset = $state(0);
	const LIMIT = 25;
	let loading = $state(false);
	let working = $state(false);
	// Chosen groups, and the record each keeps (key: kind + value).
	let chosen = $state<Record<string, boolean>>({});
	let keep = $state<Record<string, string>>({});
	const key = (g: Group) => `${g.kind}:${g.value}`;
	const KIND = { phone: 'Same phone', email: 'Same email', name: 'Same name' } as const;

	onMount(async () => {
		await crm.load();
		type = crm.type(code) ?? null;
		await load();
	});

	async function load() {
		const kinds = Object.entries(by).filter(([, v]) => v).map(([k]) => k);
		if (!kinds.length) return;
		loading = true;
		try {
			const r = await api.get<{ groups: Group[]; total: number }>(`/crm/${code}/duplicate-groups` + query({ by: kinds.join(','), q: search.trim() || undefined, limit: LIMIT, offset }));
			groups = r.groups;
			total = r.total;
			chosen = {};
			keep = Object.fromEntries(r.groups.map((g) => [key(g), g.items[0]?.id ?? '']));
		} catch (e) {
			toasts.error(e);
		} finally {
			loading = false;
		}
	}

	const picked = $derived(groups.filter((g) => chosen[key(g)]));

	async function merge(list: Group[]) {
		if (!list.length) return;
		const records = list.reduce((n, g) => n + g.items.length - 1, 0);
		if (!confirm(`Merge ${list.length} group${list.length === 1 ? '' : 's'}? ${records} record${records === 1 ? '' : 's'} will be merged into the one kept in each group; this can't be undone.`)) return;
		working = true;
		try {
			const r = await api.post<{ merged: string[]; failed: { error: string }[] }>(`/crm/${code}/duplicate-groups/merge`, {
				groups: list.map((g) => ({ item_ids: g.items.map((i) => i.id), target_id: keep[key(g)] || undefined }))
			});
			if (r.merged.length) toasts.success(`${r.merged.length} group${r.merged.length === 1 ? '' : 's'} merged`);
			for (const f of r.failed) toasts.error(f.error);
			await load();
		} catch (e) {
			toasts.error(e);
		} finally {
			working = false;
		}
	}

	async function ignore(g: Group) {
		try {
			await api.post(`/crm/${code}/duplicate-groups/ignore`, { kind: g.kind, value: g.value, item_ids: g.items.map((i) => i.id) });
			groups = groups.filter((x) => x !== g);
			total -= 1;
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Duplicates · {type?.name_plural ?? 'CRM'} · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<a href="/crm/{code}" class="btn btn-ghost p-1.5" aria-label="Back to the list"><Icon name="x" class="size-4" /></a>
		<h1 class="text-2xl font-semibold text-slate-900">Duplicate {type?.name_plural.toLowerCase() ?? 'records'}</h1>
		<div class="ml-auto flex flex-wrap items-center gap-3 text-sm">
			<input class="input w-56" type="search" placeholder="Phone, email or name…" aria-label="Search duplicates" bind:value={search} oninput={() => { clearTimeout(timer); timer = setTimeout(() => ((offset = 0), load()), 250); }} />
			<span class="text-slate-500">Look for the same</span>
			{#each ['phone', 'email', 'name'] as const as k (k)}
				<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={by[k]} onchange={() => ((offset = 0), load())} /> {k}</label>
			{/each}
		</div>
	</div>

	{#if picked.length}
		<div class="card flex items-center gap-3 px-4 py-2 text-sm" data-bulk-bar>
			<span>{picked.length} group{picked.length === 1 ? '' : 's'} chosen</span>
			<button class="btn btn-primary ml-auto" disabled={working} onclick={() => merge(picked)}>Merge chosen</button>
		</div>
	{/if}

	{#if loading && !groups.length}
		<p class="text-sm text-slate-400">Searching…</p>
	{:else if !groups.length}
		<p class="card px-5 py-10 text-center text-sm text-slate-500" data-no-duplicates>No duplicates found among the records you can see.</p>
	{:else}
		<p class="text-sm text-slate-500">{total} group{total === 1 ? '' : 's'} of look-alike records. The record kept takes in the others’ details, links, history and activities; the others are removed.</p>
		{#each groups as g (key(g))}
			<section class="card" data-dup-group={g.kind} aria-label="{KIND[g.kind]}: {g.shown}">
				<header class="flex flex-wrap items-center gap-3 border-b border-slate-100 px-4 py-2.5">
					<input type="checkbox" class="rounded border-slate-300 text-brand-600" aria-label="Choose this group" bind:checked={chosen[key(g)]} />
					<span class="badge bg-slate-100 text-slate-600">{KIND[g.kind]}</span>
					<span class="font-medium text-slate-900">{g.shown}</span>
					<span class="text-sm text-slate-500">{g.count} records</span>
					<div class="ml-auto flex gap-2">
						<button class="btn btn-ghost text-sm" onclick={() => ignore(g)} data-not-duplicates>Not duplicates</button>
						<button class="btn btn-secondary text-sm" disabled={working} onclick={() => merge([g])} data-merge-group>Merge</button>
					</div>
				</header>
				<table class="min-w-full text-sm">
					<thead class="text-left text-xs text-slate-500">
						<tr><th class="w-16 px-4 py-1.5 font-medium">Keep</th><th class="px-2 py-1.5 font-medium">Record</th><th class="px-2 py-1.5 font-medium">Phones</th><th class="px-2 py-1.5 font-medium">Emails</th><th class="px-2 py-1.5 font-medium">Responsible</th><th class="px-2 py-1.5 font-medium">Created</th><th class="px-4 py-1.5 text-right font-medium">Linked</th></tr>
					</thead>
					<tbody class="divide-y divide-slate-50">
						{#each g.items as i (i.id)}
							<tr>
								<td class="px-4 py-1.5"><input type="radio" name="keep-{key(g)}" value={i.id} bind:group={keep[key(g)]} aria-label="Keep #{i.number}" /></td>
								<td class="px-2 py-1.5"><a class="text-slate-900 hover:underline" href="/crm/{code}/{i.id}" target="_blank" rel="noopener">#{i.number} {i.title}</a></td>
								<td class="px-2 py-1.5 text-slate-600">{i.phones ?? '—'}</td>
								<td class="px-2 py-1.5 text-slate-600">{i.emails ?? '—'}</td>
								<td class="px-2 py-1.5 text-slate-600">{i.assigned_name ?? '—'}</td>
								<td class="px-2 py-1.5 text-slate-600">{formatDate(i.created_at)}</td>
								<td class="px-4 py-1.5 text-right text-slate-600 tabular-nums">{i.deals}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</section>
		{/each}
		{#if total > LIMIT}
			<div class="flex items-center justify-center gap-3 text-sm">
				<button class="btn btn-secondary" disabled={offset === 0} onclick={() => ((offset = Math.max(0, offset - LIMIT)), load())}>Previous</button>
				<span class="text-slate-500">{offset + 1}–{Math.min(offset + LIMIT, total)} of {total}</span>
				<button class="btn btn-secondary" disabled={offset + LIMIT >= total} onclick={() => ((offset += LIMIT), load())}>Next</button>
			</div>
		{/if}
	{/if}
</div>
