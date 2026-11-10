<script lang="ts">
	// Taking a test: one question at a time (as the test allows moving about), a countdown for a
	// timed test, answers saved one by one, then the result with the right answers where shown.
	import { page } from '$app/state';
	import { onDestroy } from 'svelte';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { KIND_LABEL, type Attempt, type AttemptQuestion } from '#lib/training.ts';

	const id = $derived(page.params.id);
	let a = $state<Attempt | null>(null);
	let at = $state(0);
	let busy = $state(false);
	// The person's choice for each question, before saving.
	let draft = $state<Record<string, { chosen: string[]; written: string; order: string[] }>>({});
	let left = $state<number | null>(null);
	let tick: ReturnType<typeof setInterval> | undefined;

	function take(x: Attempt) {
		a = x;
		for (const q of x.questions) {
			draft[q.question_id] ??= { chosen: [...q.chosen], written: q.written ?? '', order: q.kind === 'order' && q.chosen.length ? [...q.chosen] : q.answers.map((y) => y.id) };
		}
		if (x.current != null) at = x.current;
		left = x.seconds_left;
		clearInterval(tick);
		if (left != null && x.status === 'in_progress') {
			const end = Date.now() + left * 1000;
			tick = setInterval(() => {
				left = Math.max(0, Math.round((end - Date.now()) / 1000));
				if (left === 0) {
					clearInterval(tick);
					load();
				}
			}, 1000);
		}
	}
	async function load() {
		try {
			take(await api.get<Attempt>(`/training/attempts/${id}`));
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		id;
		load();
	});
	onDestroy(() => clearInterval(tick));

	const q = $derived(a?.questions[at]);
	const open = $derived(a?.status === 'in_progress');
	// Whether this question can still be answered (or changed).
	const editable = (x: AttemptQuestion) => open && (a!.test.navigation === 'free' || !x.answered) && (a!.test.navigation !== 'strict' || a!.current === x.position);
	const canGo = (i: number) => !open || a!.test.navigation !== 'strict' || i <= (a!.current ?? a!.questions.length - 1);

	function pick(x: AttemptQuestion, ans: string) {
		const d = draft[x.question_id];
		d.chosen = x.kind === 'single' ? [ans] : d.chosen.includes(ans) ? d.chosen.filter((y) => y !== ans) : [...d.chosen, ans];
	}
	function move(x: AttemptQuestion, i: number, by: number) {
		const d = draft[x.question_id];
		const list = [...d.order];
		const [y] = list.splice(i, 1);
		list.splice(i + by, 0, y);
		d.order = list;
	}
	async function save() {
		if (!q) return;
		const d = draft[q.question_id];
		busy = true;
		try {
			const body = q.kind === 'text' ? { question_id: q.question_id, written: d.written } : { question_id: q.question_id, chosen: q.kind === 'order' ? d.order : d.chosen };
			const before = at;
			take(await api.post<Attempt>(`/training/attempts/${id}/answers`, body));
			// Move on unless the answer is being shown.
			if (a!.test.show_answers !== 'after_each' && before < a!.questions.length - 1 && a!.test.navigation !== 'strict') at = before + 1;
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	async function finish() {
		const missing = a!.questions.filter((x) => !x.answered).length;
		if (!confirm(missing ? `Finish? ${missing} ${missing === 1 ? 'question is' : 'questions are'} unanswered and will count as wrong.` : 'Finish the test?')) return;
		try {
			take(await api.post<Attempt>(`/training/attempts/${id}/finish`));
			clearInterval(tick);
		} catch (e) {
			toasts.error(e);
		}
	}
	const clock = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
	const textOf = (x: AttemptQuestion, aid: string) => x.answers.find((y) => y.id === aid)?.text ?? '';
	const answeredCount = $derived(a?.questions.filter((x) => x.answered).length ?? 0);
</script>

<svelte:head><title>{a?.test.title ?? 'Test'} · Training · NERP</title></svelte:head>

{#if a}
	<div class="max-w-3xl space-y-4">
		<div>
			<a href="/training/courses/{a.course.id}" class="text-sm text-slate-500 hover:underline">‹ {a.course.title}</a>
			<div class="mt-1 flex flex-wrap items-center gap-3">
				<h1 class="flex-1 text-2xl font-semibold text-slate-900">{a.test.title}</h1>
				{#if open && left != null}<span class="rounded-full px-3 py-1 font-mono text-sm {left < 60 ? 'bg-red-100 text-red-700' : 'bg-slate-100 text-slate-700'}" data-clock>⏱ {clock(left)}</span>{/if}
			</div>
		</div>

		{#if !open}
			<div class="card p-5" data-result={a.status}>
				{#if a.status === 'review'}
					<p class="text-lg font-medium text-slate-900">Your written answers are being graded.</p>
					<p class="text-sm text-slate-500">You'll be notified with your result.</p>
				{:else}
					<div class="flex flex-wrap items-center gap-4">
						<span class="text-4xl font-semibold tabular-nums {a.passed ? 'text-emerald-700' : 'text-amber-700'}" data-percent>{a.percent} %</span>
						<div class="flex-1">
							<p class="text-lg font-medium" data-passed={a.passed}>{a.passed ? 'Passed' : 'Not passed'}{a.mark ? ` · ${a.mark}` : ''}</p>
							<p class="text-sm text-slate-500">{a.score} of {a.max_score} points · pass at {a.test.pass_percent} %</p>
						</div>
						<a class="btn btn-secondary" href="/training/courses/{a.course.id}">Back to the course</a>
					</div>
				{/if}
			</div>
		{/if}

		<nav class="flex flex-wrap gap-1.5" aria-label="Questions">
			{#each a.questions as x, i (x.question_id)}
				<button
					class="size-9 rounded-lg border text-sm tabular-nums {i === at ? 'border-brand-500 ring-2 ring-brand-200' : 'border-slate-200'} {x.correct === true ? 'bg-emerald-50 text-emerald-800' : x.correct === false ? 'bg-red-50 text-red-700' : x.answered ? 'bg-slate-100' : 'bg-white'}"
					disabled={!canGo(i)}
					aria-label="Question {i + 1}{x.answered ? ', answered' : ''}"
					aria-current={i === at}
					onclick={() => (at = i)}>{i + 1}</button
				>
			{/each}
			{#if open}<span class="ml-auto self-center text-sm text-slate-500">{answeredCount} of {a.questions.length} answered</span>{/if}
		</nav>

		{#if q}
			{@const d = draft[q.question_id]}
			{@const can = editable(q)}
			<section class="card space-y-4 p-5" data-question={q.position}>
				<p class="text-xs text-slate-500">Question {at + 1} · {KIND_LABEL[q.kind]} · {q.points} points{q.required_correct ? ' · must be right' : ''}</p>
				<div class="prose-kb">{@html q.html}</div>

				{#if q.kind === 'text'}
					<textarea class="input" rows="6" maxlength="5000" aria-label="Your answer" disabled={!can} bind:value={d.written}></textarea>
				{:else if q.kind === 'order'}
					<ol class="space-y-1.5">
						{#each d.order as aid, i (aid)}
							<li class="flex items-center gap-2 rounded-lg border px-3 py-2 text-sm {q.right_answers && q.right_answers[i] === aid ? 'border-emerald-400 bg-emerald-50' : q.right_answers ? 'border-red-300 bg-red-50' : 'border-slate-200'}" data-item={textOf(q, aid)}>
								<span class="w-5 text-slate-400">{i + 1}.</span><span class="flex-1">{textOf(q, aid)}</span>
								{#if can}<button class="btn btn-ghost px-2 text-xs" aria-label="Move up: {textOf(q, aid)}" disabled={i === 0} onclick={() => move(q, i, -1)}>↑</button><button class="btn btn-ghost px-2 text-xs" aria-label="Move down: {textOf(q, aid)}" disabled={i === d.order.length - 1} onclick={() => move(q, i, 1)}>↓</button>{/if}
							</li>
						{/each}
					</ol>
					{#if q.right_answers}<p class="text-sm text-slate-600">Right order: {q.right_answers.map((x) => textOf(q, x)).join(' → ')}</p>{/if}
				{:else}
					<div class="space-y-1.5">
						{#each q.answers as ans (ans.id)}
							<label class="flex items-center gap-2 rounded-lg border px-3 py-2 text-sm {q.right_answers?.includes(ans.id) ? 'border-emerald-400 bg-emerald-50' : q.right_answers && d.chosen.includes(ans.id) ? 'border-red-300 bg-red-50' : 'border-slate-200'}">
								<input type={q.kind === 'single' ? 'radio' : 'checkbox'} name="q-{q.question_id}" disabled={!can} checked={d.chosen.includes(ans.id)} onchange={() => pick(q, ans.id)} />{ans.text}
							</label>
						{/each}
					</div>
				{/if}

				{#if q.correct != null && (q.right_answers || !open)}<p class="text-sm font-medium {q.correct ? 'text-emerald-700' : 'text-red-700'}" data-verdict>{q.correct ? 'Right' : 'Wrong'}{q.earned != null ? ` · ${q.earned} of ${q.points} points` : ''}</p>{/if}
				{#if q.explanation}<p class="rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-900">{q.explanation}</p>{/if}
				{#if q.review_note}<p class="rounded-lg bg-slate-50 px-3 py-2 text-sm text-slate-700">Grader: {q.review_note}</p>{/if}

				<div class="flex flex-wrap items-center gap-2 border-t border-slate-100 pt-3">
					{#if at > 0 && canGo(at - 1)}<button class="btn btn-ghost" onclick={() => at--}>‹ Previous</button>{/if}
					<div class="flex-1"></div>
					{#if can}<button class="btn btn-primary" disabled={busy || (q.kind === 'text' ? !d.written.trim() : q.kind !== 'order' && !d.chosen.length)} onclick={save}>{q.answered ? 'Change answer' : 'Answer'}</button>{/if}
					{#if at < a.questions.length - 1 && canGo(at + 1) && (!open || a.test.navigation !== 'strict' || q.answered)}<button class="btn btn-secondary" onclick={() => at++}>Next ›</button>{/if}
				</div>
			</section>
		{/if}

		{#if open}<div class="flex justify-end"><button class="btn btn-primary" onclick={finish}>Finish the test</button></div>{/if}
	</div>
{/if}
