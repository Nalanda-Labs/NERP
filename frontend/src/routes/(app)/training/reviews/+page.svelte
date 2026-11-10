<script lang="ts">
	// Grading written answers (people who manage training): each answer waiting, right or not,
	// the points and a note for the person.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { day } from '#lib/training.ts';

	type Item = { attempt_id: string; question_id: string; html: string; points: number; written: string; test: { id: string; title: string }; course: { id: string; title: string }; person: string; finished_at: string };
	let items = $state<Item[]>([]);
	let loaded = $state(false);
	let grade = $state<Record<string, { points: number; note: string }>>({});
	const key = (i: Item) => `${i.attempt_id}:${i.question_id}`;
	async function load() {
		try {
			items = (await api.get<{ items: Item[] }>('/training/reviews')).items;
			for (const i of items) grade[key(i)] ??= { points: i.points, note: '' };
		} catch (e) {
			toasts.error(e);
		} finally {
			loaded = true;
		}
	}
	onMount(load);
	async function decide(i: Item, correct: boolean) {
		const g = grade[key(i)];
		try {
			await api.post(`/training/attempts/${i.attempt_id}/review`, { question_id: i.question_id, correct, points: correct ? g.points : 0, note: g.note || null });
			items = items.filter((x) => key(x) !== key(i));
			toasts.success('Graded');
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Grading · Training · NERP</title></svelte:head>

<div class="max-w-4xl space-y-4">
	<div><a href="/training" class="text-sm text-slate-500 hover:underline">‹ Training</a><h1 class="mt-1 text-2xl font-semibold text-slate-900">Written answers to grade</h1></div>
	{#each items as i (key(i))}
		{@const g = grade[key(i)]}
		<section class="card space-y-3 p-5" data-review={i.person}>
			<p class="text-sm text-slate-500"><span class="font-medium text-slate-800">{i.person}</span> · {i.test.title} · <a class="hover:underline" href="/training/courses/{i.course.id}">{i.course.title}</a> · {day(i.finished_at)}</p>
			<div class="prose-kb text-slate-700">{@html i.html}</div>
			<blockquote class="rounded-xl bg-slate-50 px-4 py-3 whitespace-pre-line text-slate-900" data-written>{i.written}</blockquote>
			<div class="flex flex-wrap items-end gap-3">
				<div><label class="label" for="pts-{key(i)}">Points if right (of {i.points})</label><input id="pts-{key(i)}" class="input w-24" type="number" min="0" max={i.points} bind:value={g.points} /></div>
				<div class="min-w-48 flex-1"><label class="label" for="note-{key(i)}">Note for {i.person.split(' ')[0]} (optional)</label><input id="note-{key(i)}" class="input" maxlength="2000" bind:value={g.note} /></div>
				<button class="btn btn-secondary" onclick={() => decide(i, false)}>Wrong</button>
				<button class="btn btn-primary" onclick={() => decide(i, true)}>Right</button>
			</div>
		</section>
	{:else}
		{#if loaded}<p class="card px-4 py-10 text-center text-slate-500">Nothing to grade.</p>{/if}
	{/each}
</div>
