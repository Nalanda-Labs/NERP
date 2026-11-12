<script lang="ts">
	// One personal data request: what was found per module, the report, and for erasure the
	// reviewed choice per source.
	import { page } from '$app/state';
	import { api, downloadUrl } from '#lib/api.ts';
	import { formatDate, formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Action = 'delete' | 'anonymise' | 'keep';
	type Req = { id: string; kind: 'access' | 'erasure'; email: string; phone: string | null; name: string | null; message: string | null; source: string; status: string; due_at: string | null; created_at: string; decided_by_name: string | null; decided_at: string | null; outcome: Record<string, { action: Action; count: number }> | null; note: string | null };
	type Src = { key: string; title: string; count: number; actions: Action[]; note: string; sample: string[] };
	let req = $state<Req | null>(null);
	let sources = $state<Src[]>([]);
	let choice = $state<Record<string, Action>>({});
	let confirm = $state('');
	let note = $state('');
	let busy = $state(false);

	async function load() {
		try {
			const r = await api.get<{ request: Req; sources: Src[] }>(`/privacy/requests/${page.params.id}`);
			req = r.request;
			sources = r.sources;
			choice = Object.fromEntries(r.sources.map((s) => [s.key, s.actions[0]]));
			note = r.request.note ?? '';
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		page.params.id;
		load();
	});
	async function setStatus(status: string) {
		try {
			req = await api.patch<Req>(`/privacy/requests/${req!.id}`, { status, note });
			toasts.success(status === 'done' ? 'Marked as answered' : status === 'rejected' ? 'Rejected' : 'Reopened');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function erase() {
		busy = true;
		try {
			req = await api.post<Req>(`/privacy/requests/${req!.id}/erase`, { actions: choice, confirm, note });
			sources = [];
			toasts.success('Done');
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	const ACT: Record<Action, string> = { delete: 'Delete', anonymise: 'Anonymise', keep: 'Keep' };
	const found = $derived(sources.filter((s) => s.count > 0));
</script>

<svelte:head><title>Data request · NERP</title></svelte:head>

{#if req}
	<div class="max-w-4xl space-y-5">
		<p class="text-sm"><a class="text-brand-700 hover:underline" href="/privacy">← Data requests</a></p>
		<div class="card space-y-1 p-5 text-sm">
			<h1 class="text-xl font-semibold text-slate-900">{req.kind === 'erasure' ? 'Delete the data of' : 'A copy of the data of'} {req.name ?? req.email}</h1>
			<p class="text-slate-600">{req.email}{req.phone ? ` · ${req.phone}` : ''} · received {formatDate(req.created_at)}{req.source === 'public' ? ' through the form' : ''}{req.due_at && req.status === 'open' ? ` · due ${formatDate(req.due_at)}` : ''}</p>
			{#if req.message}<p class="text-slate-500">“{req.message}”</p>{/if}
			<p class="font-medium" data-status={req.status}>{req.status === 'unverified' ? 'Waiting for the person to confirm it from the email.' : req.status === 'done' ? `Done ${formatDateTime(req.decided_at!)} by ${req.decided_by_name ?? 'someone'}` : req.status === 'rejected' ? `Rejected ${formatDateTime(req.decided_at!)}` : 'Open'}</p>
		</div>

		{#if req.outcome}
			<section class="card p-5 text-sm" aria-label="Outcome">
				<h2 class="font-semibold text-slate-900">What was done</h2>
				<ul class="mt-2 space-y-0.5" data-outcome>
					{#each Object.entries(req.outcome) as [k, o] (k)}<li><b class="font-medium">{k}</b>: {ACT[o.action]}{o.action !== 'keep' ? ` (${o.count})` : ''}</li>{/each}
				</ul>
				{#if req.note}<p class="mt-2 text-slate-500">{req.note}</p>{/if}
			</section>
		{/if}

		{#if req.status !== 'done' && req.status !== 'unverified'}
			<section class="card p-5 text-sm" aria-label="Found">
				<div class="flex flex-wrap items-center gap-2">
					<h2 class="font-semibold text-slate-900">Found in {found.length} {found.length === 1 ? 'place' : 'places'}</h2>
					<button class="btn btn-secondary ml-auto" onclick={() => downloadUrl(`/api/privacy/requests/${req!.id}/report`, `personal-data-${req!.email}.json`)} data-report>Download the report</button>
				</div>
				<ul class="mt-3 divide-y divide-slate-100">
					{#each found as s (s.key)}
						<li class="flex flex-wrap items-start gap-3 py-2.5" data-source={s.key}>
							<div class="min-w-0 flex-1">
								<p class="font-medium text-slate-900">{s.title} <span class="text-slate-500">({s.count})</span></p>
								<p class="text-xs text-slate-500">{s.sample.join(' · ')}{s.count > s.sample.length ? ' …' : ''}</p>
								{#if s.note && req.kind === 'erasure'}<p class="text-xs text-slate-400">{s.note}</p>{/if}
							</div>
							{#if req.kind === 'erasure' && req.status === 'open'}
								<select class="input w-36" bind:value={choice[s.key]} aria-label="What to do with {s.title}" disabled={s.actions.length === 1}>
									{#each s.actions as a (a)}<option value={a}>{ACT[a]}</option>{/each}
								</select>
							{/if}
						</li>
					{:else}<li class="py-2 text-slate-500">Nothing found for this person.</li>{/each}
				</ul>
			</section>

			<section class="card space-y-3 p-5 text-sm" aria-label="Decide">
				<label class="block">Note (what was sent, why kept or rejected)<textarea class="input mt-1" rows="2" bind:value={note}></textarea></label>
				{#if req.kind === 'erasure' && req.status === 'open'}
					<label class="block">Type <b>{req.email}</b> to confirm<input class="input mt-1" bind:value={confirm} data-confirm /></label>
					<div class="flex flex-wrap gap-2">
						<button class="btn btn-danger" disabled={busy || confirm.trim().toLowerCase() !== req.email.toLowerCase()} onclick={erase} data-erase>{busy ? 'Erasing…' : 'Erase as chosen'}</button>
						<button class="btn btn-secondary" onclick={() => setStatus('rejected')}>Reject</button>
					</div>
				{:else if req.status === 'open'}
					<div class="flex flex-wrap gap-2">
						<button class="btn btn-primary" onclick={() => setStatus('done')} data-answered>Report sent: mark as answered</button>
						<button class="btn btn-secondary" onclick={() => setStatus('rejected')}>Reject</button>
					</div>
				{:else if req.status === 'rejected'}
					<button class="btn btn-secondary" onclick={() => setStatus('open')}>Reopen</button>
				{/if}
			</section>
		{:else if req.status === 'unverified'}
			<button class="btn btn-secondary" onclick={() => setStatus('rejected')}>Reject</button>
		{/if}
	</div>
{:else}
	<p class="text-sm text-slate-400">Loading…</p>
{/if}
