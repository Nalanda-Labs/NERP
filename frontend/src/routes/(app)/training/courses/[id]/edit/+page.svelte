<script lang="ts">
	// Editing a course (people who manage training): its details and who may take it; chapters
	// and lessons; questions; tests and their marks.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, fileUrl } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { toasts } from '#lib/toast.svelte.ts';
	import { KIND_LABEL, NAVIGATION, SHOW_ANSWERS, type CourseView, type QuestionKind } from '#lib/training.ts';

	const id = $derived(page.params.id);
	let v = $state<CourseView | null>(null);
	let tab = $state<'course' | 'lessons' | 'questions' | 'tests'>('course');
	let departments = $state<{ id: string; name: string }[]>([]);

	// The course's details.
	let form = $state({ title: '', description: '', is_published: false, to_all: true, cover_file_id: null as string | null });
	let audienceUsers = $state<PickedUser[]>([]);
	let audienceDepts = $state<string[]>([]);
	let preview = $state('');
	function fill(c: CourseView) {
		v = c;
		form = { title: c.course.title, description: c.course.description, is_published: c.course.is_published, to_all: c.course.to_all, cover_file_id: c.course.cover_file_id };
		audienceUsers = (c.audience?.users ?? []).map((u) => ({ id: u.id, name: u.name, avatar_file_id: u.avatar_file_id ?? null }));
		audienceDepts = c.audience?.departments ?? [];
	}
	async function load() {
		try {
			fill(await api.get<CourseView>(`/training/courses/${id}`));
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(() => {
		load();
		api.get<{ id: string; name: string }[]>('/departments').then((d) => (departments = d)).catch(() => {});
	});
	async function saveCourse(e: SubmitEvent) {
		e.preventDefault();
		try {
			fill(await api.patch<CourseView>(`/training/courses/${id}`, { ...form, audience: { users: audienceUsers.map((u) => u.id), departments: audienceDepts } }));
			toasts.success('Saved');
		} catch (err) {
			toasts.error(err);
		}
	}
	async function showPreview() {
		preview = (await api.post<{ html: string }>('/kb/preview', { body: form.description })).html;
	}
	async function uploadCover(e: Event) {
		const f = (e.currentTarget as HTMLInputElement).files?.[0];
		if (!f) return;
		const fd = new FormData();
		fd.append('public', 'true');
		fd.append('file', f, f.name);
		try {
			form.cover_file_id = (await api.post<{ id: string }>('/files', fd)).id;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeCourse() {
		if (!confirm('Delete this course with its lessons, tests, results and certificates?')) return;
		try {
			await api.del(`/training/courses/${id}`);
			goto('/training');
		} catch (err) {
			toasts.error(err);
		}
	}

	// Chapters and lessons.
	let chapterTitle = $state('');
	async function addChapter(e: SubmitEvent) {
		e.preventDefault();
		try {
			fill(await api.post<CourseView>(`/training/courses/${id}/chapters`, { title: chapterTitle }));
			chapterTitle = '';
		} catch (err) {
			toasts.error(err);
		}
	}
	async function renameChapter(c: { id: string; title: string }) {
		const t = prompt('Chapter title', c.title);
		if (!t?.trim()) return;
		fill(await api.patch<CourseView>(`/training/chapters/${c.id}`, { title: t }));
	}
	async function moveChapter(i: number, by: number) {
		const list = [...v!.chapters];
		const [c] = list.splice(i, 1);
		list.splice(i + by, 0, c);
		for (const [n, ch] of list.entries()) await api.patch(`/training/chapters/${ch.id}`, { sort: (n + 1) * 10 });
		await load();
	}
	async function deleteChapter(c: { id: string; title: string }) {
		if (!confirm(`Delete the chapter “${c.title}”? Its lessons stay, outside any chapter.`)) return;
		fill(await api.del<CourseView>(`/training/chapters/${c.id}`));
	}
	async function addLesson(chapter: string | null) {
		const t = prompt('Lesson title');
		if (!t?.trim()) return;
		try {
			const l = await api.post<{ id: string }>(`/training/courses/${id}/lessons`, { title: t, chapter_id: chapter });
			goto(`/training/lessons/${l.id}?edit=1`);
		} catch (err) {
			toasts.error(err);
		}
	}
	async function moveLesson(chapter: string | null, i: number, by: number) {
		const list = v!.lessons.filter((l) => l.chapter_id === chapter);
		const [l] = list.splice(i, 1);
		list.splice(i + by, 0, l);
		for (const [n, x] of list.entries()) await api.patch(`/training/lessons/${x.id}`, { sort: (n + 1) * 10 });
		await load();
	}

	// Questions.
	type Q = { id: string; lesson_id: string | null; lesson_title: string | null; kind: QuestionKind; text: string; points: number; required_correct: boolean; explanation: string | null; self_check: boolean; answers: { id?: string; text: string; is_correct: boolean }[] };
	let questions = $state<Q[]>([]);
	const loadQuestions = async () => (questions = (await api.get<{ items: Q[] }>(`/training/courses/${id}/questions`)).items);
	let qOpen = $state(false);
	let qEdit = $state<Q | null>(null);
	let qf = $state<Omit<Q, 'id' | 'lesson_title'>>({ lesson_id: null, kind: 'single', text: '', points: 10, required_correct: false, explanation: null, self_check: false, answers: [] });
	function editQuestion(q: Q | null) {
		qEdit = q;
		qf = q
			? { lesson_id: q.lesson_id, kind: q.kind, text: q.text, points: q.points, required_correct: q.required_correct, explanation: q.explanation, self_check: q.self_check, answers: q.answers.map((a) => ({ text: a.text, is_correct: a.is_correct })) }
			: { lesson_id: null, kind: 'single', text: '', points: 10, required_correct: false, explanation: null, self_check: false, answers: [{ text: '', is_correct: true }, { text: '', is_correct: false }] };
		qOpen = true;
	}
	function setKind(k: QuestionKind) {
		qf.kind = k;
		if (k === 'text') qf.answers = [];
		else if (qf.answers.length < 2) qf.answers = [{ text: '', is_correct: k !== 'order' }, { text: '', is_correct: false }];
		if (k === 'single') {
			const first = qf.answers.findIndex((a) => a.is_correct);
			qf.answers = qf.answers.map((a, i) => ({ ...a, is_correct: i === Math.max(first, 0) }));
		}
	}
	async function saveQuestion(e: SubmitEvent) {
		e.preventDefault();
		try {
			const r = qEdit ? await api.put<{ items: Q[] }>(`/training/questions/${qEdit.id}`, qf) : await api.post<{ items: Q[] }>(`/training/courses/${id}/questions`, qf);
			questions = r.items;
			qOpen = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function deleteQuestion(q: Q) {
		if (!confirm('Delete this question?')) return;
		try {
			questions = (await api.del<{ items: Q[] }>(`/training/questions/${q.id}`)).items;
		} catch (err) {
			toasts.error(err);
		}
	}

	// Tests.
	type T = {
		id?: string; title: string; description: string; is_active: boolean; lesson_id: string | null; question_count: number; shuffle_questions: boolean; shuffle_answers: boolean;
		navigation: string; show_answers: string; attempt_limit: number; time_limit_minutes: number; wait_minutes: number; pass_percent: number;
		prerequisite_test_id: string | null; prerequisite_percent: number; marks: { min_percent: number; mark: string }[]; pool?: number;
	};
	let tOpen = $state(false);
	let tf = $state<T>(blankTest());
	function blankTest(): T {
		return { title: '', description: '', is_active: true, lesson_id: null, question_count: 0, shuffle_questions: true, shuffle_answers: true, navigation: 'free', show_answers: 'at_end', attempt_limit: 0, time_limit_minutes: 0, wait_minutes: 0, pass_percent: 70, prerequisite_test_id: null, prerequisite_percent: 0, marks: [] };
	}
	async function editTest(tid: string | null) {
		tf = tid ? await api.get<T>(`/training/tests/${tid}`) : blankTest();
		tOpen = true;
	}
	async function saveTest(e: SubmitEvent) {
		e.preventDefault();
		try {
			if (tf.id) await api.put(`/training/tests/${tf.id}`, tf);
			else await api.post(`/training/courses/${id}/tests`, tf);
			tOpen = false;
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function deleteTest(tid: string) {
		if (!confirm('Delete this test?')) return;
		try {
			await api.del(`/training/tests/${tid}`);
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}
	$effect(() => {
		if (tab === 'questions' && !questions.length) loadQuestions().catch(() => {});
	});
	const lessonsOf = (chapter: string | null) => v?.lessons.filter((l) => l.chapter_id === chapter) ?? [];
</script>

<svelte:head><title>Edit {v?.course.title ?? 'course'} · Training · NERP</title></svelte:head>

{#if v}
	<div class="max-w-5xl space-y-4">
		<div>
			<a href="/training/courses/{id}" class="text-sm text-slate-500 hover:underline">‹ {v.course.title}</a>
			<div class="mt-1 flex flex-wrap items-center gap-3">
				<h1 class="flex-1 text-2xl font-semibold text-slate-900">Edit course</h1>
				<span class="rounded-full px-2.5 py-1 text-xs {v.course.is_published ? 'bg-emerald-100 text-emerald-800' : 'bg-amber-100 text-amber-800'}" data-published={v.course.is_published}>{v.course.is_published ? 'Published' : 'Draft'}</span>
			</div>
		</div>
		<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist" style="width:fit-content">
			{#each [['course', 'Course'], ['lessons', 'Lessons'], ['questions', 'Questions'], ['tests', 'Tests']] as [k, label] (k)}
				<button role="tab" aria-selected={tab === k} class="rounded-full px-4 py-1.5 font-medium {tab === k ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = k as typeof tab)}>{label}</button>
			{/each}
		</div>

		{#if tab === 'course'}
			<form class="card space-y-4 p-5" onsubmit={saveCourse}>
				<div><label class="label" for="c-title">Title</label><input id="c-title" class="input" required maxlength="200" bind:value={form.title} /></div>
				<div>
					<div class="flex items-center"><label class="label flex-1" for="c-desc">About the course (Markdown)</label><button type="button" class="text-xs text-brand-600 hover:underline" onclick={showPreview}>Preview</button></div>
					<textarea id="c-desc" class="input font-mono text-sm" rows="8" bind:value={form.description}></textarea>
					{#if preview}<div class="prose-kb mt-2 rounded-xl bg-slate-50 p-3">{@html preview}</div>{/if}
				</div>
				<div class="flex items-center gap-3">
					{#if form.cover_file_id}<img src={fileUrl(form.cover_file_id)} alt="Cover" class="h-16 w-28 rounded-lg object-cover" />{/if}
					<label class="btn btn-secondary cursor-pointer text-sm">{form.cover_file_id ? 'Change cover' : 'Add a cover picture'}<input type="file" accept="image/*" class="hidden" onchange={uploadCover} /></label>
					{#if form.cover_file_id}<button type="button" class="text-sm text-slate-500 hover:underline" onclick={() => (form.cover_file_id = null)}>Remove</button>{/if}
				</div>
				<div>
					<span class="label">Who may enrol</span>
					<select class="input" aria-label="Who may enrol" bind:value={form.to_all}><option value={true}>Everyone</option><option value={false}>Chosen people and departments</option></select>
					{#if !form.to_all}
						<div class="mt-2 space-y-2 rounded-xl bg-slate-50 p-3">
							<MultiUserPicker bind:value={audienceUsers} placeholder="People…" />
							<div class="flex flex-wrap gap-1.5">{#each departments as d (d.id)}<button type="button" class="rounded-full border px-3 py-1 text-sm {audienceDepts.includes(d.id) ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-600'}" aria-pressed={audienceDepts.includes(d.id)} onclick={() => (audienceDepts = audienceDepts.includes(d.id) ? audienceDepts.filter((x) => x !== d.id) : [...audienceDepts, d.id])}>{d.name}</button>{/each}</div>
						</div>
					{/if}
					<p class="mt-1 text-xs text-slate-500">You can also assign it to anyone, with a due date, from the course page.</p>
				</div>
				<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={form.is_published} /> Published: people can see it and enrol</label>
				<div class="flex gap-2"><button class="btn btn-primary">Save</button><div class="flex-1"></div><button type="button" class="btn btn-ghost text-red-600" onclick={removeCourse}>Delete course</button></div>
			</form>
		{:else if tab === 'lessons'}
			<div class="space-y-3">
				{#each [null, ...v.chapters.map((c) => c.id)] as chId, ci (chId ?? 'none')}
					{@const ch = v.chapters.find((c) => c.id === chId)}
					{#if chId === null && !lessonsOf(null).length && v.chapters.length}{:else}
						<section class="card" data-chapter={ch?.title ?? 'none'}>
							<div class="flex items-center gap-2 border-b border-slate-100 px-4 py-2.5">
								<h2 class="flex-1 font-medium">{ch?.title ?? 'Lessons'}</h2>
								{#if ch}
									<button class="btn btn-ghost px-2 text-xs" aria-label="Move up" disabled={ci === 1} onclick={() => moveChapter(ci - 1, -1)}>↑</button>
									<button class="btn btn-ghost px-2 text-xs" aria-label="Move down" disabled={ci === v.chapters.length} onclick={() => moveChapter(ci - 1, 1)}>↓</button>
									<button class="btn btn-ghost text-xs" onclick={() => renameChapter(ch)}>Rename</button>
									<button class="btn btn-ghost text-xs text-red-600" onclick={() => deleteChapter(ch)}>Delete</button>
								{/if}
								<button class="btn btn-secondary text-xs" onclick={() => addLesson(chId)}>Add a lesson</button>
							</div>
							<ul class="divide-y divide-slate-50">
								{#each lessonsOf(chId) as l, i (l.id)}
									<li class="flex items-center gap-2 px-4 py-2">
										<a class="flex-1 hover:underline" href="/training/lessons/{l.id}?edit=1">{l.title}</a>
										{#if l.opens_after_days}<span class="text-xs text-slate-400">opens on day {l.opens_after_days}</span>{/if}
										<button class="btn btn-ghost px-2 text-xs" aria-label="Move up" disabled={i === 0} onclick={() => moveLesson(chId, i, -1)}>↑</button>
										<button class="btn btn-ghost px-2 text-xs" aria-label="Move down" disabled={i === lessonsOf(chId).length - 1} onclick={() => moveLesson(chId, i, 1)}>↓</button>
									</li>
								{:else}
									<li class="px-4 py-3 text-sm text-slate-400">No lessons yet.</li>
								{/each}
							</ul>
						</section>
					{/if}
				{/each}
				<form class="flex gap-2" onsubmit={addChapter}>
					<input class="input max-w-xs" placeholder="New chapter" aria-label="New chapter" required maxlength="200" bind:value={chapterTitle} />
					<button class="btn btn-secondary">Add chapter</button>
				</form>
			</div>
		{:else if tab === 'questions'}
			<div class="space-y-3">
				<div class="flex items-center"><p class="flex-1 text-sm text-slate-500">Tests draw from these questions. Self-check questions appear in their lesson instead.</p><button class="btn btn-primary text-sm" onclick={() => editQuestion(null)}>Add a question</button></div>
				<div class="card divide-y divide-slate-50">
					{#each questions as q (q.id)}
						<div class="flex items-start gap-3 px-4 py-3" data-question={q.text}>
							<div class="flex-1">
								<p class="text-slate-900">{q.text}</p>
								<p class="text-xs text-slate-500">{KIND_LABEL[q.kind]} · {q.points} points{q.lesson_title ? ` · ${q.lesson_title}` : ''}{q.self_check ? ' · self-check' : ''}{q.required_correct ? ' · must be right' : ''}</p>
							</div>
							<button class="btn btn-ghost text-xs" onclick={() => editQuestion(q)}>Edit</button>
							<button class="btn btn-ghost text-xs text-red-600" onclick={() => deleteQuestion(q)}>Delete</button>
						</div>
					{:else}
						<p class="px-4 py-8 text-center text-sm text-slate-500">No questions yet.</p>
					{/each}
				</div>
			</div>
		{:else}
			<div class="space-y-3">
				<div class="flex justify-end"><button class="btn btn-primary text-sm" onclick={() => editTest(null)}>Add a test</button></div>
				<div class="card divide-y divide-slate-50">
					{#each v.tests as t (t.id)}
						<div class="flex items-center gap-3 px-4 py-3" data-test={t.title}>
							<div class="flex-1"><p class="font-medium text-slate-900">{t.title}{#if !t.is_active}<span class="ml-2 text-xs font-normal text-slate-400">off</span>{/if}</p><p class="text-xs text-slate-500">{t.questions} questions · pass at {t.pass_percent} %</p></div>
							<button class="btn btn-ghost text-xs" onclick={() => editTest(t.id)}>Edit</button>
							<button class="btn btn-ghost text-xs text-red-600" onclick={() => deleteTest(t.id)}>Delete</button>
						</div>
					{:else}
						<p class="px-4 py-8 text-center text-sm text-slate-500">No tests: the course is completed when every lesson is done.</p>
					{/each}
				</div>
			</div>
		{/if}
	</div>
{/if}

<Modal bind:open={qOpen} title={qEdit ? 'Edit question' : 'New question'} size="lg">
	<form id="q-form" class="space-y-3" onsubmit={saveQuestion}>
		<div class="flex flex-wrap gap-2" role="radiogroup" aria-label="Kind">
			{#each Object.entries(KIND_LABEL) as [k, label] (k)}
				<button type="button" role="radio" aria-checked={qf.kind === k} class="rounded-full border px-3 py-1.5 text-sm {qf.kind === k ? 'border-brand-500 bg-brand-50 font-medium text-brand-700' : 'border-slate-200 text-slate-700'}" onclick={() => setKind(k as QuestionKind)}>{label}</button>
			{/each}
		</div>
		<div><label class="label" for="q-text">Question (Markdown)</label><textarea id="q-text" class="input" rows="3" required bind:value={qf.text}></textarea></div>
		{#if qf.kind !== 'text'}
			<div class="space-y-2">
				<span class="label">{qf.kind === 'order' ? 'Items, in their right order' : 'Answers (tick the right ones)'}</span>
				{#each qf.answers as a, i (i)}
					<div class="flex items-center gap-2">
						{#if qf.kind === 'single'}<input type="radio" name="right" aria-label="Right answer {i + 1}" checked={a.is_correct} onchange={() => (qf.answers = qf.answers.map((x, j) => ({ ...x, is_correct: j === i })))} />
						{:else if qf.kind === 'multiple'}<input type="checkbox" aria-label="Right answer {i + 1}" bind:checked={a.is_correct} />
						{:else}<span class="w-5 text-right text-sm text-slate-400">{i + 1}.</span>{/if}
						<input class="input flex-1" aria-label="Answer {i + 1}" required maxlength="2000" bind:value={a.text} />
						<button type="button" class="btn btn-ghost px-2 text-xs" aria-label="Remove answer {i + 1}" disabled={qf.answers.length <= 2} onclick={() => (qf.answers = qf.answers.filter((_, j) => j !== i))}>✕</button>
					</div>
				{/each}
				<button type="button" class="text-sm text-brand-600 hover:underline" onclick={() => (qf.answers = [...qf.answers, { text: '', is_correct: false }])}>Add {qf.kind === 'order' ? 'an item' : 'an answer'}</button>
			</div>
		{:else}
			<p class="text-sm text-slate-500">People write their answer; someone who manages training grades it.</p>
		{/if}
		<div class="grid gap-3 sm:grid-cols-2">
			<div><label class="label" for="q-lesson">Lesson</label><select id="q-lesson" class="input" bind:value={qf.lesson_id}><option value={null}>— the whole course —</option>{#each v?.lessons ?? [] as l (l.id)}<option value={l.id}>{l.title}</option>{/each}</select></div>
			<div><label class="label" for="q-pts">Points</label><input id="q-pts" class="input" type="number" min="0" max="1000" bind:value={qf.points} /></div>
		</div>
		<div><label class="label" for="q-expl">Explanation shown when answered wrongly</label><input id="q-expl" class="input" maxlength="2000" bind:value={qf.explanation} /></div>
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={qf.required_correct} /> Must be answered right to pass, whatever the score</label>
		{#if qf.kind !== 'text'}<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={qf.self_check} /> Self-check in its lesson (not used in tests)</label>{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (qOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="q-form">Save</button>
	{/snippet}
</Modal>

<Modal bind:open={tOpen} title={tf.id ? 'Edit test' : 'New test'} size="lg">
	<form id="t-form" class="space-y-3" onsubmit={saveTest}>
		<div><label class="label" for="t-title">Title</label><input id="t-title" class="input" required maxlength="200" bind:value={tf.title} /></div>
		<div><label class="label" for="t-desc">Description (Markdown)</label><textarea id="t-desc" class="input" rows="2" bind:value={tf.description}></textarea></div>
		<div class="grid gap-3 sm:grid-cols-2">
			<div><label class="label" for="t-lesson">Questions from</label><select id="t-lesson" class="input" bind:value={tf.lesson_id}><option value={null}>the whole course</option>{#each v?.lessons ?? [] as l (l.id)}<option value={l.id}>{l.title}</option>{/each}</select></div>
			<div><label class="label" for="t-count">How many (0: all{tf.pool != null ? ` ${tf.pool}` : ''})</label><input id="t-count" class="input" type="number" min="0" bind:value={tf.question_count} /></div>
			<div><label class="label" for="t-pass">Pass at (%)</label><input id="t-pass" class="input" type="number" min="0" max="100" bind:value={tf.pass_percent} /></div>
			<div><label class="label" for="t-time">Time limit (minutes, 0: none)</label><input id="t-time" class="input" type="number" min="0" max="1440" bind:value={tf.time_limit_minutes} /></div>
			<div><label class="label" for="t-att">Attempts (0: unlimited)</label><input id="t-att" class="input" type="number" min="0" bind:value={tf.attempt_limit} /></div>
			<div><label class="label" for="t-wait">Wait between attempts (minutes)</label><input id="t-wait" class="input" type="number" min="0" bind:value={tf.wait_minutes} /></div>
			<div><label class="label" for="t-nav">Moving through it</label><select id="t-nav" class="input" bind:value={tf.navigation}>{#each NAVIGATION as n (n.value)}<option value={n.value}>{n.label}</option>{/each}</select></div>
			<div><label class="label" for="t-show">Show the right answers</label><select id="t-show" class="input" bind:value={tf.show_answers}>{#each SHOW_ANSWERS as n (n.value)}<option value={n.value}>{n.label}</option>{/each}</select></div>
			<div><label class="label" for="t-pre">Taken after passing</label><select id="t-pre" class="input" bind:value={tf.prerequisite_test_id}><option value={null}>—</option>{#each (v?.tests ?? []).filter((x) => x.id !== tf.id) as x (x.id)}<option value={x.id}>{x.title}</option>{/each}</select></div>
			{#if tf.prerequisite_test_id}<div><label class="label" for="t-prep">with at least (%)</label><input id="t-prep" class="input" type="number" min="0" max="100" bind:value={tf.prerequisite_percent} /></div>{/if}
		</div>
		<div class="flex flex-wrap gap-4 text-sm">
			<label class="flex items-center gap-2"><input type="checkbox" bind:checked={tf.shuffle_questions} /> Shuffle questions</label>
			<label class="flex items-center gap-2"><input type="checkbox" bind:checked={tf.shuffle_answers} /> Shuffle answers</label>
			<label class="flex items-center gap-2"><input type="checkbox" bind:checked={tf.is_active} /> On</label>
		</div>
		<div class="space-y-2">
			<span class="label">Marks</span>
			{#each tf.marks as m, i (i)}
				<div class="flex items-center gap-2"><span class="text-sm text-slate-500">from</span><input class="input w-20" type="number" min="0" max="100" aria-label="Mark {i + 1} from" bind:value={m.min_percent} /><span class="text-sm text-slate-500">%</span><input class="input flex-1" aria-label="Mark {i + 1}" maxlength="50" placeholder="Excellent" bind:value={m.mark} /><button type="button" class="btn btn-ghost px-2 text-xs" aria-label="Remove mark" onclick={() => (tf.marks = tf.marks.filter((_, j) => j !== i))}>✕</button></div>
			{/each}
			<button type="button" class="text-sm text-brand-600 hover:underline" onclick={() => (tf.marks = [...tf.marks, { min_percent: 90, mark: '' }])}>Add a mark</button>
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (tOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="t-form">Save</button>
	{/snippet}
</Modal>
