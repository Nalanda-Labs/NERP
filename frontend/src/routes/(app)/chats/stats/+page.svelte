<script lang="ts">
	// Live chat figures: chats, answered and missed, how fast the first answer came, how long
	// chats lasted, visitors' ratings and reviews; per chat button, per operator and per day.
	import { api, query, requestBlob } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Fig = { chats: number; answered: number; missed: number; offline: number; first_reply_secs: number | null; duration_secs: number | null; rating: number | null; ratings: number; review: number | null; name?: string | null };
	type Stats = { total: Fig; channels: Fig[]; operators: Fig[]; days: { day: string; chats: number; answered: number }[] };
	const iso = (d: Date) => d.toISOString().slice(0, 10);
	let from = $state(iso(new Date(Date.now() - 29 * 86400e3)));
	let to = $state(iso(new Date()));
	let channel = $state('');
	let channels = $state<{ id: string; name: string }[]>([]);
	let stats = $state<Stats | null>(null);

	const params = () => ({ from: new Date(`${from}T00:00:00`).toISOString(), to: new Date(new Date(`${to}T00:00:00`).getTime() + 86400e3).toISOString(), channel_id: channel || undefined });
	$effect(() => {
		from;
		to;
		channel;
		api.get<Stats>('/chat/stats' + query(params())).then((s) => (stats = s)).catch(toasts.error);
	});
	$effect(() => {
		api.get<{ items: { id: string; name: string }[] }>('/chat/channels').then((r) => (channels = r.items)).catch(() => {});
	});
	const dur = (s: number | null) => (s === null ? '—' : s < 90 ? `${Math.round(s)} s` : s < 5400 ? `${Math.round(s / 60)} min` : `${(s / 3600).toFixed(1)} h`);
	const mark = (v: number | null) => (v === null ? '—' : v.toFixed(1));
	async function exportCsv() {
		try {
			const blob = await requestBlob('GET', '/chat/export.csv' + query(params()));
			const a = document.createElement('a');
			a.href = URL.createObjectURL(blob);
			a.download = `chats-${from}-${to}.csv`;
			a.click();
			URL.revokeObjectURL(a.href);
		} catch (e) {
			toasts.error(e);
		}
	}
	const maxDay = $derived(Math.max(1, ...(stats?.days.map((d) => d.chats) ?? [1])));
</script>

<svelte:head><title>Chat figures · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-end gap-3">
		<div class="mr-auto"><a href="/chats" class="text-sm text-brand-600 hover:underline">‹ Chats</a><h1 class="text-2xl font-semibold text-slate-900">Chat figures</h1></div>
		<label class="text-sm">From <input class="input" type="date" bind:value={from} /></label>
		<label class="text-sm">To <input class="input" type="date" bind:value={to} /></label>
		<select class="input w-48" aria-label="Chat button" bind:value={channel}><option value="">All chat buttons</option>{#each channels as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</select>
		<button class="btn btn-secondary" onclick={exportCsv}>Export CSV</button>
	</div>
	{#if stats}
		<div class="grid gap-3 sm:grid-cols-3 lg:grid-cols-6" data-totals>
			{#each [['Chats', String(stats.total.chats)], ['Answered', String(stats.total.answered)], ['Missed', String(stats.total.missed)], ['First answer', dur(stats.total.first_reply_secs)], ['Length', dur(stats.total.duration_secs)], ['Rating', `${mark(stats.total.rating)} (${stats.total.ratings})`]] as [l, v] (l)}
				<div class="card p-4"><p class="text-xs text-slate-500">{l}</p><p class="mt-1 text-2xl font-semibold text-slate-900 tabular-nums">{v}</p></div>
			{/each}
		</div>
		<section class="card p-4">
			<h2 class="mb-3 font-medium text-slate-900">Chats per day</h2>
			<div class="flex h-32 items-end gap-1" role="img" aria-label="Chats per day">
				{#each stats.days as d (d.day)}
					<div class="flex flex-1 flex-col justify-end" title="{d.day}: {d.chats} chats, {d.answered} answered">
						<div class="rounded-t bg-brand-600" style="height:{(d.answered / maxDay) * 100}%"></div>
						<div class="bg-slate-300" style="height:{((d.chats - d.answered) / maxDay) * 100}%"></div>
					</div>
				{/each}
			</div>
			<p class="mt-2 text-xs text-slate-500"><span class="mr-1 inline-block size-2 bg-brand-600"></span>answered <span class="mr-1 ml-3 inline-block size-2 bg-slate-300"></span>missed</p>
		</section>
		{#each [['By chat button', stats.channels], ['By operator', stats.operators]] as [title, rows] (title)}
			<section class="card overflow-x-auto">
				<h2 class="px-4 pt-4 font-medium text-slate-900">{title}</h2>
				<table class="mt-2 w-full text-sm">
					<thead class="text-left text-xs text-slate-500"><tr><th class="px-4 py-2"></th><th class="px-2">Chats</th><th class="px-2">Answered</th><th class="px-2">Missed</th><th class="px-2">First answer</th><th class="px-2">Length</th><th class="px-2">Rating</th><th class="px-2">Review</th></tr></thead>
					<tbody class="divide-y divide-slate-100 tabular-nums">
						{#each rows as Fig[] as r (r.name)}
							<tr><td class="px-4 py-2 font-medium">{r.name ?? '—'}</td><td class="px-2">{r.chats}</td><td class="px-2">{r.answered}</td><td class="px-2">{r.missed}</td><td class="px-2">{dur(r.first_reply_secs)}</td><td class="px-2">{dur(r.duration_secs)}</td><td class="px-2">{mark(r.rating)}</td><td class="px-2">{mark(r.review)}</td></tr>
						{:else}<tr><td class="px-4 py-3 text-slate-400" colspan="8">No chats.</td></tr>{/each}
					</tbody>
				</table>
			</section>
		{/each}
	{/if}
</div>
