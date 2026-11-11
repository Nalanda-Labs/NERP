<script lang="ts">
	// Work reports: mine (the current one to write and send, and past ones) and those sent to
	// me to read and mark.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Report = { id: string; user_name: string; period: string; period_start: string; period_end: string; body: string | null; plans: string | null; status: string; mark: string | null; sent_at: string | null; figures: { tasks_done: { number: number; title: string }[]; worked_minutes: number; task_minutes: number; meetings: number } };
	let tab = $state<'mine' | 'review'>('mine');
	let items = $state<Report[]>([]);
	let current = $state<{ period: string; readers: string[]; report: Report | null } | null>(null);
	let text = $state({ body: '', plans: '' });

	const range = (r: Report) => (r.period === 'daily' ? new Date(r.period_start).toLocaleDateString() : `${new Date(r.period_start).toLocaleDateString()} – ${new Date(r.period_end).toLocaleDateString()}`);
	const MARK: Record<string, string> = { good: '👍 Good', bad: '👎 Needs work', none: 'Read' };
	async function load() {
		items = (await api.get<{ items: Report[] }>(`/hr/reports?scope=${tab}`)).items;
	}
	onMount(async () => {
		current = await api.get('/hr/reports/current');
		if (current?.report) text = { body: current.report.body ?? '', plans: current.report.plans ?? '' };
		load();
	});
	async function save(send: boolean) {
		if (!current?.report) return;
		try {
			let r = await api.put<Report>(`/hr/reports/${current.report.id}`, text);
			if (send) r = await api.post<Report>(`/hr/reports/${r.id}/send`, {});
			current = { ...current, report: r };
			toasts.success(send ? 'Report sent' : 'Saved');
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Work reports · NERP</title></svelte:head>

<div class="max-w-4xl space-y-4">
	<h1 class="text-2xl font-semibold text-slate-900">Work reports</h1>
	{#if current?.report && current.report.status === 'draft'}
		<section class="card space-y-3 p-5" aria-label="Current report" data-current>
			<div class="flex flex-wrap items-baseline gap-2">
				<h2 class="font-medium text-slate-900">{current.period === 'daily' ? "Today's" : current.period === 'weekly' ? "This week's" : "This month's"} report</h2>
				<span class="text-sm text-slate-500">{range(current.report)} · for {current.readers.join(', ') || 'nobody yet'}</span>
			</div>
			<p class="text-sm text-slate-600">{current.report.figures.tasks_done.length} tasks done · {Math.round(current.report.figures.worked_minutes / 6) / 10} h worked · {Math.round(current.report.figures.task_minutes / 6) / 10} h on tasks · {current.report.figures.meetings} meetings</p>
			{#if current.report.figures.tasks_done.length}<ul class="text-sm text-slate-600">{#each current.report.figures.tasks_done as t (t.number)}<li>✓ #{t.number} {t.title}</li>{/each}</ul>{/if}
			<textarea class="input" rows="4" aria-label="What I did" placeholder="What I did" bind:value={text.body}></textarea>
			<textarea class="input" rows="2" aria-label="Plans" placeholder="What comes next" bind:value={text.plans}></textarea>
			<div class="flex justify-end gap-2"><button class="btn btn-secondary" onclick={() => save(false)}>Save</button><button class="btn btn-primary" disabled={!text.body.trim() || !current.readers.length} onclick={() => save(true)}>Send</button></div>
		</section>
	{:else if current && current.period === 'none'}
		<p class="text-sm text-slate-500">You don't have to send work reports. You can still write a day's report from <a class="text-brand-600 hover:underline" href="/hr/my-day">My day</a>.</p>
	{/if}
	<div class="flex rounded-full bg-slate-100 p-0.5 w-fit" role="tablist">
		{#each [['mine', 'Mine'], ['review', 'To read']] as [v, l] (v)}<button role="tab" aria-selected={tab === v} class="rounded-full px-3 py-1 text-sm font-medium {tab === v ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => ((tab = v as typeof tab), load())}>{l}</button>{/each}
	</div>
	<ul class="card divide-y divide-slate-100">
		{#each items as r (r.id)}
			<li><a class="flex flex-wrap items-center gap-3 px-5 py-3 hover:bg-slate-50" href="/hr/reports/{r.id}" data-report={r.user_name}>
				<span class="font-medium text-slate-900">{tab === 'review' ? r.user_name : range(r)}</span>
				{#if tab === 'review'}<span class="text-sm text-slate-500">{range(r)}</span>{/if}
				<span class="ml-auto text-sm {r.status === 'sent' ? 'text-amber-700' : 'text-slate-500'}">{r.status === 'draft' ? 'Draft' : r.status === 'sent' ? (tab === 'review' ? 'To read' : 'Sent') : MARK[r.mark ?? 'none']}</span>
				{#if r.sent_at}<span class="text-xs text-slate-400">{formatDateTime(r.sent_at)}</span>{/if}
			</a></li>
		{:else}<li class="px-5 py-6 text-center text-sm text-slate-400">No reports.</li>{/each}
	</ul>
</div>
