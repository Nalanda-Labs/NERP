<script lang="ts">
	// A course: what it is about, enrolling, its chapters and lessons, its tests and where I stand,
	// and the certificate. People who manage training edit it, see the gradebook and assign it.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { toasts } from '#lib/toast.svelte.ts';
	import { day, type CourseView, type TestItem } from '#lib/training.ts';

	let v = $state<CourseView | null>(null);
	const id = $derived(page.params.id);
	async function load() {
		try {
			v = await api.get<CourseView>(`/training/courses/${id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		id;
		load();
	});

	async function enrol() {
		try {
			v = await api.post<CourseView>(`/training/courses/${id}/enrol`);
			toasts.success('Enrolled');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function leave() {
		if (!confirm('Leave this course? Your progress is kept if you come back.')) return;
		try {
			await api.del(`/training/courses/${id}/enrol`);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function startTest(t: TestItem) {
		try {
			const a = await api.post<{ id: string }>(`/training/tests/${t.id}/attempts`);
			goto(`/training/attempts/${a.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}

	// Lessons grouped by chapter (lessons outside chapters first).
	const groups = $derived.by(() => {
		if (!v) return [];
		const out: { id: string | null; title: string | null; lessons: CourseView['lessons'] }[] = [];
		const loose = v.lessons.filter((l) => !l.chapter_id);
		if (loose.length) out.push({ id: null, title: null, lessons: loose });
		for (const c of v.chapters) out.push({ id: c.id, title: c.title, lessons: v.lessons.filter((l) => l.chapter_id === c.id) });
		return out;
	});

	// Assigning (managers).
	let assignOpen = $state(false);
	let assignUsers = $state<PickedUser[]>([]);
	let assignDepts = $state<string[]>([]);
	let dueOn = $state('');
	let departments = $state<{ id: string; name: string }[]>([]);
	async function openAssign() {
		assignUsers = [];
		assignDepts = [];
		dueOn = '';
		assignOpen = true;
		if (!departments.length) departments = await api.get<{ id: string; name: string }[]>('/departments').catch(() => []);
	}
	async function assign(e: SubmitEvent) {
		e.preventDefault();
		try {
			const r = await api.post<{ assigned: number; already_completed: number }>(`/training/courses/${id}/assign`, { users: assignUsers.map((u) => u.id), departments: assignDepts, due_on: dueOn || null });
			toasts.success(`Assigned to ${r.assigned} ${r.assigned === 1 ? 'person' : 'people'}${r.already_completed ? ` (${r.already_completed} already completed it)` : ''}`);
			assignOpen = false;
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>{v?.course.title ?? 'Course'} · Training · NERP</title></svelte:head>

{#if v}
	<div class="max-w-5xl space-y-5">
		<div>
			<a href="/training" class="text-sm text-slate-500 hover:underline">‹ Training</a>
			<div class="mt-1 flex flex-wrap items-start gap-3">
				<h1 class="flex-1 text-2xl font-semibold text-slate-900">{v.course.title}</h1>
				{#if !v.course.is_published}<span class="rounded-full bg-amber-100 px-2.5 py-1 text-xs text-amber-800">Draft</span>{/if}
				{#if v.can_manage}
					<a class="btn btn-ghost text-sm" href="/training/courses/{id}/gradebook">Gradebook ({v.enrolled})</a>
					{#if v.course.is_published}<button class="btn btn-secondary text-sm" onclick={openAssign}>Assign</button>{/if}
					<a class="btn btn-secondary text-sm" href="/training/courses/{id}/edit">Edit</a>
				{/if}
			</div>
		</div>

		<div class="card flex flex-wrap items-center gap-4 p-4" data-enrolment>
			{#if v.certificate}
				<span class="text-3xl" aria-hidden="true">🎓</span>
				<div class="flex-1"><p class="font-medium text-slate-900">Completed {v.enrolment?.completed_at ? day(v.enrolment.completed_at) : ''}</p><p class="text-sm text-slate-500">Certificate no. {v.certificate.number}</p></div>
				<a class="btn btn-primary" href="/training/certificates/{v.certificate.id}">Certificate</a>
			{:else if v.enrolment && v.progress}
				<div class="flex-1 space-y-1.5">
					<p class="text-sm text-slate-600">
						{v.progress.lessons_done} of {v.progress.lessons} lessons · {v.progress.tests_passed} of {v.progress.tests} tests passed
						{#if v.enrolment.due_on}<span class={!v.enrolment.completed_at && v.enrolment.due_on < new Date().toISOString().slice(0, 10) ? 'font-medium text-red-600' : ''}> · due {day(v.enrolment.due_on)}</span>{/if}
						{#if v.enrolment.source === 'assigned'}<span class="text-slate-400"> · assigned to you</span>{/if}
					</p>
					<div class="h-2 overflow-hidden rounded-full bg-slate-100"><div class="h-full rounded-full bg-brand-500" style="width: {v.progress.percent}%" data-progress={v.progress.percent}></div></div>
				</div>
				{#if v.enrolment.source === 'self' && !v.enrolment.completed_at}<button class="btn btn-ghost text-sm" onclick={leave}>Leave</button>{/if}
			{:else if v.can_enrol}
				<p class="flex-1 text-sm text-slate-600">Enrol to read the lessons, take the tests and earn the certificate.</p>
				<button class="btn btn-primary" onclick={enrol}>Enrol</button>
			{:else}
				<p class="flex-1 text-sm text-slate-600">{v.course.is_published ? 'This course is for chosen people and departments.' : 'Publish the course so people can enrol.'}</p>
			{/if}
		</div>

		{#if v.html}<div class="prose-kb max-w-3xl">{@html v.html}</div>{/if}

		<div class="grid gap-5 lg:grid-cols-[1fr_22rem]">
			<section class="card">
				<h2 class="border-b border-slate-100 px-4 py-3 font-medium">Lessons</h2>
				{#each groups as g (g.id)}
					{#if g.title}<h3 class="bg-slate-50 px-4 py-2 text-xs font-semibold tracking-wide text-slate-500 uppercase">{g.title}</h3>{/if}
					<ul class="divide-y divide-slate-50">
						{#each g.lessons as l (l.id)}
							<li data-lesson={l.title}>
								{#if l.open}
									<a class="flex items-center gap-3 px-4 py-2.5 hover:bg-slate-50" href="/training/lessons/{l.id}">
										<span class="flex size-5 items-center justify-center rounded-full text-xs {l.done ? 'bg-emerald-500 text-white' : 'border border-slate-300'}" aria-label={l.done ? 'done' : 'not done'}>{l.done ? '✓' : ''}</span>
										<span class="flex-1">{l.title}</span>
										{#if v.can_manage && l.opens_after_days}<span class="text-xs text-slate-400">opens on day {l.opens_after_days}</span>{/if}
									</a>
								{:else}
									<div class="flex items-center gap-3 px-4 py-2.5 text-slate-400" title={l.opens_at ? `Opens ${day(l.opens_at)}` : 'Enrol to read'}>
										<span aria-hidden="true">🔒</span><span class="flex-1">{l.title}</span>{#if l.opens_at}<span class="text-xs">opens {day(l.opens_at)}</span>{/if}
									</div>
								{/if}
							</li>
						{:else}
							<li class="px-4 py-3 text-sm text-slate-400">No lessons in this chapter yet.</li>
						{/each}
					</ul>
				{:else}
					<p class="px-4 py-8 text-center text-sm text-slate-500">No lessons yet.</p>
				{/each}
			</section>

			<section class="space-y-3">
				<h2 class="font-medium">Tests</h2>
				{#each v.tests as t (t.id)}
					<div class="card space-y-2 p-4" data-test={t.title}>
						<div class="flex items-start gap-2">
							<h3 class="flex-1 font-medium text-slate-900">{t.title}</h3>
							{#if t.passed}<span class="rounded-full bg-emerald-100 px-2 py-0.5 text-xs text-emerald-800">Passed</span>{/if}
							{#if !t.is_active}<span class="rounded-full bg-slate-100 px-2 py-0.5 text-xs text-slate-600">Off</span>{/if}
						</div>
						<p class="text-xs text-slate-500">
							{t.questions} questions · pass at {t.pass_percent} %{t.time_limit_minutes ? ` · ${t.time_limit_minutes} min` : ''}{t.attempt_limit ? ` · ${t.attempt_limit} ${t.attempt_limit === 1 ? 'attempt' : 'attempts'}` : ''}
						</p>
						{#if t.best_percent != null}<p class="text-sm" data-best>Best: <span class="font-medium tabular-nums">{t.best_percent} %</span>{#if t.attempts_left != null} · {t.attempts_left} {t.attempts_left === 1 ? 'attempt' : 'attempts'} left{/if}</p>{/if}
						{#if t.waiting_review}<p class="text-sm text-amber-700">Written answers are being graded.</p>{/if}
						{#if v.enrolment}
							{#if t.in_progress}
								<a class="btn btn-primary w-full text-sm" href="/training/attempts/{t.in_progress}">Continue</a>
							{:else if t.can_start}
								<button class="btn {t.passed ? 'btn-secondary' : 'btn-primary'} w-full text-sm" onclick={() => startTest(t)}>{t.attempts_used ? 'Try again' : 'Start'}</button>
							{:else if t.why_not}
								<p class="text-xs text-slate-500" data-why-not>{t.why_not[0].toUpperCase() + t.why_not.slice(1)}.</p>
							{/if}
						{/if}
					</div>
				{:else}
					<p class="card px-4 py-6 text-center text-sm text-slate-500">No tests: the course is completed when every lesson is done.</p>
				{/each}
			</section>
		</div>
	</div>
{/if}

<Modal bind:open={assignOpen} title="Assign the course">
	<form id="assign" class="space-y-3" onsubmit={assign}>
		<MultiUserPicker bind:value={assignUsers} placeholder="People…" />
		<div class="flex flex-wrap gap-1.5">
			{#each departments as d (d.id)}<button type="button" class="rounded-full border px-3 py-1 text-sm {assignDepts.includes(d.id) ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-600'}" aria-pressed={assignDepts.includes(d.id)} onclick={() => (assignDepts = assignDepts.includes(d.id) ? assignDepts.filter((x) => x !== d.id) : [...assignDepts, d.id])}>{d.name}</button>{/each}
		</div>
		<div><label class="label" for="as-due">Due (optional)</label><input id="as-due" class="input w-48" type="date" bind:value={dueOn} /></div>
		<p class="text-xs text-slate-500">They are notified, and reminded two days before it's due. A department includes its sub-departments.</p>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (assignOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="assign">Assign</button>
	{/snippet}
</Modal>
