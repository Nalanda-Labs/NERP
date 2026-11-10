<script lang="ts">
	// The minutes of a meeting, ready to read or print: who was there, each topic with its
	// reports, decision and task, and the notes.
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import { meetingTime, ROLE_LABEL, STATE_LABEL, tree, type Meeting } from '#lib/meetings.ts';

	const id = $derived(page.params.id!);
	let m = $state<Meeting | null>(null);
	let missing = $state(false);
	$effect(() => {
		void id;
		api.get<Meeting>(`/meetings/${id}`).then((x) => (m = x)).catch(() => (missing = true));
	});
	const present = $derived(m?.members.filter((x) => x.attended === true) ?? []);
	const absent = $derived(m?.members.filter((x) => x.attended === false) ?? []);
	const status = { open: 'open', done: 'settled', carried: 'carried to the next meeting' } as const;
</script>

<svelte:head><title>Minutes · {m?.title ?? 'Meeting'} · NERP</title></svelte:head>

{#if missing}
	<p class="card px-4 py-10 text-center text-slate-500">This meeting doesn't exist or isn't yours to see.</p>
{:else if m}
	<div class="mx-auto max-w-3xl space-y-6 print:max-w-none">
		<div class="flex items-center justify-between print:hidden">
			<a href="/meetings/{m.id}" class="text-sm text-slate-500 hover:underline">‹ {m.title}</a>
			<button class="btn btn-secondary" onclick={() => window.print()}>Print</button>
		</div>
		<header class="border-b border-slate-200 pb-4">
			<p class="text-sm text-slate-500">Minutes{m.state !== 'closed' ? ` (draft: ${STATE_LABEL[m.state].toLowerCase()})` : ''}</p>
			<h1 class="text-2xl font-semibold text-slate-900">{m.title}</h1>
			<p class="text-slate-600">{meetingTime(m.starts_at, m.duration_minutes)}{m.place ? ` · ${m.place}` : ''}{m.project ? ` · ${m.project.name}` : ''}</p>
		</header>

		<section>
			<h2 class="mb-1 font-semibold text-slate-900">People</h2>
			<ul class="text-sm text-slate-700">
				{#each m.members.filter((x) => x.role !== 'member') as p (p.id)}<li>{ROLE_LABEL[p.role]}: {p.name}</li>{/each}
			</ul>
			{#if present.length || absent.length}
				<p class="mt-1 text-sm text-slate-700" data-attendance><span class="font-medium">Present:</span> {present.map((p) => p.name).join(', ') || '—'}</p>
				{#if absent.length}<p class="text-sm text-slate-700"><span class="font-medium">Absent:</span> {absent.map((p) => p.name).join(', ')}</p>{/if}
			{:else}
				<p class="mt-1 text-sm text-slate-700"><span class="font-medium">Invited:</span> {m.members.map((p) => p.name).join(', ')}</p>
			{/if}
		</section>

		<section class="space-y-4">
			<h2 class="font-semibold text-slate-900">Agenda and decisions</h2>
			{#each tree(m.agenda) as t, i (t.id)}
				{#each [t, ...t.subs] as item, j (item.id)}
					<div class={j ? 'ml-6' : ''} data-minutes-topic={item.title}>
						<h3 class="font-medium text-slate-900">{j ? `${i + 1}.${j}` : `${i + 1}.`} {item.title} <span class="text-xs font-normal text-slate-500">({status[item.status]})</span></h3>
						{#if item.responsible.length}<p class="text-xs text-slate-500">Presented by {item.responsible.map((p) => p.name).join(', ')}</p>{/if}
						{#each item.reports as r (r.user_id)}<p class="mt-1 text-sm whitespace-pre-line text-slate-700"><span class="font-medium">{r.name}:</span> {r.body}</p>{/each}
						<p class="mt-1 text-sm text-slate-900"><span class="font-medium">Decision:</span> {item.decision ?? '—'}</p>
						{#if item.task}<p class="text-sm text-slate-700">Task #{item.task.number}: {item.task.title}</p>{/if}
					</div>
				{/each}
			{:else}
				<p class="text-sm text-slate-500">No topics.</p>
			{/each}
		</section>

		{#if m.minutes}
			<section>
				<h2 class="mb-1 font-semibold text-slate-900">Notes</h2>
				<p class="text-sm whitespace-pre-line text-slate-700" data-minutes-notes>{m.minutes}</p>
			</section>
		{/if}
		{#if m.next}<p class="text-sm text-slate-600">Next meeting: {meetingTime(m.next.starts_at)}.</p>{/if}
	</div>
{/if}
