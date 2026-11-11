<script lang="ts">
	// The feed's "Request": start one of the company's processes (a list whose processes run
	// on new items: leave, expenses, purchases…) by filling in its form. The request is posted
	// to the feed for whoever has to approve it, who can decide right there.
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Modal from './Modal.svelte';
	import UserPicker from './UserPicker.svelte';
	import type { Post } from './FeedPost.svelte';

	let { open = $bindable(false), onposted }: { open?: boolean; onposted?: (p: Post) => void } = $props();
	type Field = { code: string; label: string; kind: string; options: string[]; required: boolean };
	let processes = $state<{ id: string; name: string; description: string | null }[]>([]);
	let listId = $state('');
	let fields = $state<Field[]>([]);
	let title = $state('');
	let note = $state('');
	let values = $state<Record<string, unknown>>({});
	let labels = $state<Record<string, string>>({});
	let busy = $state(false);
	$effect(() => {
		if (!open) return;
		api.get<{ items: typeof processes }>('/feed/processes').then((r) => {
			processes = r.items;
			if (!listId && r.items[0]) choose(r.items[0].id);
		});
	});
	async function choose(id: string) {
		listId = id;
		values = {};
		labels = {};
		fields = id ? (await api.get<{ fields: Field[] }>(`/lists/${id}`)).fields.filter((f) => f.kind !== 'crm') : [];
	}
	async function upload(e: Event, code: string) {
		const f = (e.currentTarget as HTMLInputElement).files?.[0];
		if (!f) return;
		const fd = new FormData();
		fd.append('file', f, f.name);
		const up = await api.post<{ id: string }>('/files', fd);
		values[code] = up.id;
		labels[code] = f.name;
	}
	async function send(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const clean = Object.fromEntries(Object.entries(values).filter(([, v]) => v !== '' && v !== null && v !== undefined));
			const p = await api.post<Post>('/feed/requests', { list_id: listId, title, values: clean, note: note || null });
			onposted?.(p);
			open = false;
			title = '';
			note = '';
			values = {};
			toasts.success('Request sent');
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

<Modal bind:open title="New request">
	{#if !processes.length}
		<p class="text-sm text-slate-500">There are no processes you can start yet. They are set up under Lists (with a process that starts when an item is added).</p>
	{:else}
		<form id="request-form" class="space-y-3 text-sm" onsubmit={send}>
			<div>
				<label class="label" for="rq-list">What</label>
				<select id="rq-list" class="input" value={listId} onchange={(e) => choose(e.currentTarget.value)}>{#each processes as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select>
				{#if processes.find((p) => p.id === listId)?.description}<p class="mt-1 text-xs text-slate-500">{processes.find((p) => p.id === listId)?.description}</p>{/if}
			</div>
			<div><label class="label" for="rq-title">Title</label><input id="rq-title" class="input" required maxlength="300" bind:value={title} /></div>
			{#each fields as f (f.code)}
				<div>
					<label class="label" for="rq-{f.code}">{f.label}{f.required ? ' *' : ''}</label>
					{#if f.kind === 'textarea'}<textarea id="rq-{f.code}" class="input" rows="3" required={f.required} bind:value={values[f.code]}></textarea>
					{:else if f.kind === 'number' || f.kind === 'money'}<input id="rq-{f.code}" class="input" type="number" step="any" required={f.required} bind:value={values[f.code]} />
					{:else if f.kind === 'date'}<input id="rq-{f.code}" class="input" type="date" required={f.required} bind:value={values[f.code]} />
					{:else if f.kind === 'select'}<select id="rq-{f.code}" class="input" required={f.required} bind:value={values[f.code]}><option value={null}>—</option>{#each f.options as o (o)}<option>{o}</option>{/each}</select>
					{:else if f.kind === 'checkbox'}<input id="rq-{f.code}" type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={values[f.code] as boolean} />
					{:else if f.kind === 'user'}
						{#if values[f.code]}<p>{labels[f.code]} <button type="button" class="ml-2 text-slate-400 hover:text-red-600" onclick={() => (values[f.code] = null)}>change</button></p>
						{:else}<UserPicker onpick={(u) => ((values[f.code] = u.id), (labels[f.code] = u.full_name))} />{/if}
					{:else if f.kind === 'file'}
						{#if values[f.code]}<p>{labels[f.code]} <button type="button" class="ml-2 text-slate-400 hover:text-red-600" onclick={() => (values[f.code] = null)}>remove</button></p>
						{:else}<input id="rq-{f.code}" type="file" onchange={(e) => upload(e, f.code)} />{/if}
					{:else}<input id="rq-{f.code}" class="input" required={f.required} bind:value={values[f.code]} />{/if}
				</div>
			{/each}
			<div><label class="label" for="rq-note">A few words for the post <span class="font-normal text-slate-400">(optional)</span></label><textarea id="rq-note" class="input" rows="2" maxlength="2000" bind:value={note}></textarea></div>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		{#if processes.length}<button class="btn btn-primary" form="request-form" disabled={busy || !listId}>Send</button>{/if}
	{/snippet}
</Modal>
