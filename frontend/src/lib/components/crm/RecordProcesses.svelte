<script lang="ts">
	// A CRM record's business processes: running and finished ones with what they did, and
	// starting one by hand.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { itemId, canWrite }: { itemId: string; canWrite: boolean } = $props();
	type P = { id: string; name: string; status: string; started_at: string; error: string | null; log: { text: string; user: string | null; at: string }[]; approvals: { id: string; title: string; status: string }[] };
	let processes = $state<P[]>([]);
	let canStart = $state<{ id: string; name: string }[]>([]);
	let open = $state<string | null>(null);
	let pick = $state('');
	const STATUS: Record<string, string> = { running: 'Running', waiting: 'Waiting', completed: 'Done', rejected: 'Rejected', cancelled: 'Cancelled', failed: 'Stopped' };
	async function load() {
		const r = await api.get<{ processes: P[]; can_start: typeof canStart }>(`/crm/items/${itemId}/workflows`).catch(() => null);
		if (r) {
			processes = r.processes;
			canStart = r.can_start;
		}
	}
	onMount(load);
	async function start() {
		if (!pick) return;
		try {
			const r = await api.post<{ processes: P[]; can_start: typeof canStart }>(`/crm/items/${itemId}/workflows/${pick}`);
			processes = r.processes;
			pick = '';
			toasts.success('Started');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function cancel(p: P) {
		if (!confirm(`Cancel “${p.name}”?`)) return;
		await api.post(`/workflows/instances/${p.id}/cancel`).catch(toasts.error);
		load();
	}
</script>

{#if processes.length || (canWrite && canStart.length)}
	<section class="card space-y-3 p-5" aria-labelledby="proc-h" data-record-processes>
		<h2 id="proc-h" class="font-medium text-slate-900">Business processes</h2>
		{#if canWrite && canStart.length}
			<div class="flex gap-2">
				<select class="input flex-1" aria-label="Process to start" bind:value={pick}><option value="">Start a process…</option>{#each canStart as t (t.id)}<option value={t.id}>{t.name}</option>{/each}</select>
				<button class="btn btn-secondary" disabled={!pick} onclick={start} data-start-process>Start</button>
			</div>
		{/if}
		<ul class="space-y-2 text-sm">
			{#each processes as p (p.id)}
				<li class="rounded-lg border border-slate-100 px-3 py-2" data-process-run={p.status}>
					<button class="flex w-full items-center gap-2 text-left" onclick={() => (open = open === p.id ? null : p.id)} aria-expanded={open === p.id}>
						<span class="flex-1 font-medium text-slate-800">{p.name}</span>
						<span class="badge {p.status === 'completed' ? 'bg-emerald-50 text-emerald-800' : p.status === 'running' || p.status === 'waiting' ? 'bg-amber-50 text-amber-800' : 'bg-slate-100 text-slate-600'}">{STATUS[p.status] ?? p.status}</span>
					</button>
					{#if open === p.id}
						<ul class="mt-2 space-y-0.5 text-xs text-slate-600">
							{#each p.log as l, i (i)}<li><span class="text-slate-400">{formatDateTime(l.at)}</span> {l.text}{l.user ? ` · ${l.user}` : ''}</li>{/each}
						</ul>
						{#if p.error}<p class="mt-1 text-xs text-red-700">{p.error}</p>{/if}
						{#if (p.status === 'running' || p.status === 'waiting') && canWrite}<button class="mt-1 text-xs text-red-700 hover:underline" onclick={() => cancel(p)}>Cancel it</button>{/if}
					{/if}
				</li>
			{/each}
		</ul>
	</section>
{/if}
