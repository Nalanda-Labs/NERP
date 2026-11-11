<script lang="ts">
	// Marketing → Sales generator: a deal (or lead) for every client in a segment of contacts or
	// companies — repeat sales, renewals, seasonal offers — now or every week, month or quarter.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { crm } from '#lib/crm.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Gen = {
		id: string; name: string; segment_id: string; segment_name: string | null; target_code: 'deal' | 'lead'; category_id: string | null; stage_id: string | null;
		title: string; assign: 'owner' | 'users'; user_ids: string[]; skip_open: boolean; repeat: string; next_run_at: string | null; last_run_at: string | null;
		last_result: { created?: number; skipped?: number; failed?: number; error?: string } | null; is_active: boolean; made: number;
	};
	type Segment = { id: string; name: string; entity_type: number; count: number };
	const REPEAT: Record<string, string> = { once: 'Once', weekly: 'Every week', monthly: 'Every month', quarterly: 'Every quarter' };

	let items = $state<Gen[]>([]);
	let segments = $state<Segment[]>([]);
	let open = $state(false);
	let saving = $state(false);
	let draft = $state({ name: '', segment_id: '', target: 'deal' as 'deal' | 'lead', category_id: '', stage_id: '', title: '{{name}}: repeat sale', assign: 'owner' as 'owner' | 'users', users: [] as PickedUser[], skip_open: true, repeat: 'once', start: 'now' as 'now' | 'later', start_at: '' });

	async function load() {
		items = (await api.get<{ items: Gen[] }>('/marketing/generators')).items;
	}
	onMount(async () => {
		crm.load();
		await load();
		segments = (await api.get<{ items: Segment[] }>('/marketing/segments')).items.filter((s) => s.entity_type === 3 || s.entity_type === 4);
	});

	const targetType = $derived(crm.config?.types.find((t) => t.code === draft.target));
	const pipeline = $derived(targetType?.categories.find((c) => c.id === draft.category_id) ?? targetType?.categories.find((c) => c.is_default));

	function create() {
		draft = { name: '', segment_id: segments[0]?.id ?? '', target: 'deal', category_id: '', stage_id: '', title: '{{name}}: repeat sale', assign: 'owner', users: [], skip_open: true, repeat: 'once', start: 'now', start_at: '' };
		open = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			await api.post('/marketing/generators', {
				name: draft.name, segment_id: draft.segment_id, target: draft.target, category_id: draft.category_id || undefined, stage_id: draft.stage_id || undefined,
				title: draft.title, assign: draft.assign, user_ids: draft.users.map((u) => u.id), skip_open: draft.skip_open, repeat: draft.repeat,
				start_at: draft.start === 'later' && draft.start_at ? new Date(draft.start_at).toISOString() : undefined
			});
			open = false;
			toasts.success(draft.start === 'now' ? 'Started: you’ll be told when the records are made' : 'Scheduled');
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
	async function runNow(g: Gen) {
		try {
			const p = await api.get<{ would_make: number; would_skip: number }>(`/marketing/generators/${g.id}/preview`);
			if (!confirm(`Make ${p.would_make} ${g.target_code}${p.would_make === 1 ? '' : 's'} now${p.would_skip ? ` (${p.would_skip} clients skipped: they already have an open one)` : ''}?`)) return;
			await api.post(`/marketing/generators/${g.id}/run`);
			toasts.success('Started');
			setTimeout(load, 2000);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function toggle(g: Gen) {
		await api.patch(`/marketing/generators/${g.id}`, { is_active: !g.is_active }).catch(toasts.error);
		await load();
	}
	async function remove(g: Gen) {
		if (!confirm(`Delete “${g.name}”? The records it made stay.`)) return;
		await api.del(`/marketing/generators/${g.id}`).catch(toasts.error);
		await load();
	}
</script>

<svelte:head><title>Sales generator · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Sales generator</h1>
		<button class="btn btn-primary ml-auto" onclick={create} disabled={!segments.length}><Icon name="plus" class="size-4" /> New generator</button>
	</div>
	<p class="text-sm text-slate-500">Make a deal or lead for every client in a segment of contacts or companies: repeat sales, renewals, seasonal offers. {#if !segments.length}Make a segment of contacts or companies first.{/if}</p>
	<section class="card divide-y divide-slate-50">
		{#each items as g (g.id)}
			<div class="flex flex-wrap items-center gap-3 px-5 py-3 text-sm {g.is_active ? '' : 'opacity-60'}" data-generator={g.name}>
				<div class="min-w-0 flex-1">
					<p class="font-medium text-slate-900">{g.name}</p>
					<p class="text-slate-500">{g.target_code === 'deal' ? 'Deals' : 'Leads'} for “{g.segment_name ?? 'a deleted segment'}” · {REPEAT[g.repeat]} · {g.made} made so far</p>
					<p class="text-xs text-slate-400">
						{#if g.last_run_at}Last run {formatDateTime(g.last_run_at)}{#if g.last_result}: {g.last_result.error ?? `${g.last_result.created ?? 0} made, ${g.last_result.skipped ?? 0} skipped${g.last_result.failed ? `, ${g.last_result.failed} failed` : ''}`}{/if}{/if}
						{#if g.next_run_at}{g.last_run_at ? ' · ' : ''}Next {formatDateTime(g.next_run_at)}{/if}
					</p>
				</div>
				<button class="btn btn-secondary" onclick={() => runNow(g)} data-run-generator>Run now</button>
				<button class="btn btn-ghost" onclick={() => toggle(g)}>{g.is_active ? 'Pause' : 'Resume'}</button>
				<button class="btn btn-ghost p-1.5 text-slate-400 hover:text-red-600" aria-label="Delete generator" onclick={() => remove(g)}><Icon name="trash" class="size-4" /></button>
			</div>
		{:else}
			<p class="px-5 py-8 text-center text-sm text-slate-500">No generators yet.</p>
		{/each}
	</section>
</div>

<Modal bind:open title="New sales generator" size="lg">
	<form id="gen-form" class="grid gap-3 sm:grid-cols-2" onsubmit={save}>
		<div class="sm:col-span-2"><label class="label" for="g-name">Name</label><input id="g-name" class="input" required placeholder="Annual renewals" bind:value={draft.name} /></div>
		<div><label class="label" for="g-seg">Clients</label><select id="g-seg" class="input" required bind:value={draft.segment_id}>{#each segments as s (s.id)}<option value={s.id}>{s.name} ({s.count})</option>{/each}</select></div>
		<div><label class="label" for="g-target">Make</label><select id="g-target" class="input" bind:value={draft.target} onchange={() => ((draft.category_id = ''), (draft.stage_id = ''))}><option value="deal">Deals</option><option value="lead">Leads</option></select></div>
		{#if targetType && targetType.categories.length > 1}
			<div><label class="label" for="g-cat">Pipeline</label><select id="g-cat" class="input" bind:value={draft.category_id} onchange={() => (draft.stage_id = '')}><option value="">Default</option>{#each targetType.categories as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</select></div>
		{/if}
		<div><label class="label" for="g-stage">Stage</label><select id="g-stage" class="input" bind:value={draft.stage_id}><option value="">First stage</option>{#each (pipeline?.stages ?? []).filter((s) => s.semantics === 'process') as s (s.id)}<option value={s.id}>{s.name}</option>{/each}</select></div>
		<div class="sm:col-span-2"><label class="label" for="g-title">Title</label><input id="g-title" class="input" required bind:value={draft.title} /><p class="mt-1 text-xs text-slate-500">{'{{name}}'} is the client; {'{{month}}'} and {'{{year}}'} the time of the run.</p></div>
		<div class="sm:col-span-2 flex flex-wrap gap-4 text-sm">
			<label class="flex items-center gap-1.5"><input type="radio" value="owner" bind:group={draft.assign} /> The client’s responsible person</label>
			<label class="flex items-center gap-1.5"><input type="radio" value="users" bind:group={draft.assign} /> These people in turn</label>
		</div>
		{#if draft.assign === 'users'}<div class="sm:col-span-2"><MultiUserPicker bind:value={draft.users} placeholder="Choose people" /></div>{/if}
		<label class="flex items-center gap-1.5 text-sm sm:col-span-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.skip_open} /> Skip clients who already have an open {draft.target} {draft.category_id ? 'in this pipeline' : ''}</label>
		<div><label class="label" for="g-rep">Repeat</label><select id="g-rep" class="input" bind:value={draft.repeat}>{#each Object.entries(REPEAT) as [k, l] (k)}<option value={k}>{l}</option>{/each}</select></div>
		<div>
			<span class="label">First run</span>
			<div class="flex items-center gap-3 text-sm">
				<label class="flex items-center gap-1.5"><input type="radio" value="now" bind:group={draft.start} /> Now</label>
				<label class="flex items-center gap-1.5"><input type="radio" value="later" bind:group={draft.start} /> At</label>
				{#if draft.start === 'later'}<input type="datetime-local" class="input" required bind:value={draft.start_at} aria-label="First run at" />{/if}
			</div>
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="gen-form" disabled={saving}>{draft.start === 'now' ? 'Save and run' : 'Save'}</button>
	{/snippet}
</Modal>
