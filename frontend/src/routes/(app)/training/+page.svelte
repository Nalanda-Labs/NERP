<script lang="ts">
	// Training: my courses (enrolled or assigned), every course I may take, my certificates; for
	// people who manage training, new courses and written answers to grade.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api, fileUrl } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';
	import { day, type CourseCard } from '#lib/training.ts';

	type Cert = { id: string; number: number; course_id: string; course_title: string; issued_at: string; score: number; max_score: number };
	let tab = $state<'mine' | 'all' | 'certs'>('mine');
	let mine = $state<CourseCard[]>([]);
	let all = $state<CourseCard[]>([]);
	let certs = $state<Cert[]>([]);
	let canManage = $state(false);
	let toGrade = $state(0);
	let loaded = $state(false);

	onMount(async () => {
		try {
			const [m, a, c] = await Promise.all([
				api.get<{ items: CourseCard[]; can_manage: boolean }>('/training/courses?filter=mine'),
				api.get<{ items: CourseCard[] }>('/training/courses'),
				api.get<{ items: Cert[] }>('/training/certificates')
			]);
			mine = m.items;
			all = a.items;
			certs = c.items;
			canManage = m.can_manage;
			if (!mine.length) tab = 'all';
			if (canManage) toGrade = (await api.get<{ items: unknown[] }>('/training/reviews')).items.length;
		} catch (e) {
			toasts.error(e);
		} finally {
			loaded = true;
		}
	});

	let newOpen = $state(false);
	let title = $state('');
	async function create(e: SubmitEvent) {
		e.preventDefault();
		try {
			const r = await api.post<{ course: { id: string } }>('/training/courses', { title });
			goto(`/training/courses/${r.course.id}/edit`);
		} catch (err) {
			toasts.error(err);
		}
	}
	const list = $derived(tab === 'mine' ? mine : all);
</script>

<svelte:head><title>Training · NERP</title></svelte:head>

<div class="max-w-6xl space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">Training</h1>
		{#if canManage}
			<a class="btn btn-ghost text-sm" href="/training/reviews" data-grading>Grading{#if toGrade}<span class="ml-1.5 rounded-full bg-red-500 px-1.5 text-xs text-white">{toGrade}</span>{/if}</a>
			<button class="btn btn-primary" onclick={() => ((title = ''), (newOpen = true))}>New course</button>
		{/if}
	</div>

	<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist" style="width:fit-content">
		<button role="tab" aria-selected={tab === 'mine'} class="rounded-full px-4 py-1.5 font-medium {tab === 'mine' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = 'mine')}>My training</button>
		<button role="tab" aria-selected={tab === 'all'} class="rounded-full px-4 py-1.5 font-medium {tab === 'all' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = 'all')}>All courses</button>
		<button role="tab" aria-selected={tab === 'certs'} class="rounded-full px-4 py-1.5 font-medium {tab === 'certs' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (tab = 'certs')}>Certificates</button>
	</div>

	{#if tab === 'certs'}
		<div class="card divide-y divide-slate-50">
			{#each certs as c (c.id)}
				<a class="flex items-center gap-4 px-4 py-3 hover:bg-slate-50" href="/training/certificates/{c.id}" data-cert={c.course_title}>
					<span class="text-2xl" aria-hidden="true">🎓</span>
					<span class="flex-1"><span class="block font-medium text-slate-900">{c.course_title}</span><span class="text-sm text-slate-500">No. {c.number} · {day(c.issued_at)}</span></span>
					{#if c.max_score}<span class="text-sm text-slate-500 tabular-nums">{Math.round((c.score * 100) / c.max_score)} %</span>{/if}
				</a>
			{:else}
				<p class="px-4 py-10 text-center text-slate-500">Complete a course to earn its certificate.</p>
			{/each}
		</div>
	{:else if loaded}
		<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
			{#each list as c (c.id)}
				<a class="card flex flex-col overflow-hidden hover:shadow-g2" href="/training/courses/{c.id}" data-course={c.title}>
					{#if c.cover_file_id}<img src={fileUrl(c.cover_file_id)} alt="" class="h-32 w-full object-cover" />{:else}<div class="flex h-32 items-center justify-center bg-gradient-to-br from-brand-50 to-slate-100 text-4xl" aria-hidden="true">🎓</div>{/if}
					<div class="flex flex-1 flex-col gap-2 p-4">
						<div class="flex items-start gap-2">
							<h2 class="flex-1 font-medium text-slate-900">{c.title}</h2>
							{#if !c.is_published}<span class="rounded-full bg-amber-100 px-2 py-0.5 text-xs text-amber-800">Draft</span>{/if}
							{#if c.certificate_id}<span title="Completed" aria-label="Completed">🎓</span>{/if}
						</div>
						{#if c.excerpt}<p class="line-clamp-2 text-sm text-slate-500">{c.excerpt.replace(/[#*_>`]/g, '')}</p>{/if}
						<p class="mt-auto text-xs text-slate-500">{c.lessons} {c.lessons === 1 ? 'lesson' : 'lessons'} · {c.tests} {c.tests === 1 ? 'test' : 'tests'}</p>
						{#if c.enrolment && c.progress}
							<div class="h-1.5 overflow-hidden rounded-full bg-slate-100"><div class="h-full rounded-full {c.enrolment.completed_at ? 'bg-emerald-500' : 'bg-brand-500'}" style="width: {c.enrolment.completed_at ? 100 : c.progress.percent}%"></div></div>
							<p class="text-xs {c.overdue ? 'font-medium text-red-600' : 'text-slate-500'}" data-status>
								{#if c.enrolment.completed_at}Completed {day(c.enrolment.completed_at)}{:else}{c.progress.percent} % done{#if c.enrolment.due_on} · {c.overdue ? 'overdue since' : 'due'} {day(c.enrolment.due_on)}{/if}{/if}
							</p>
						{/if}
					</div>
				</a>
			{:else}
				<p class="card col-span-full px-4 py-10 text-center text-slate-500">{tab === 'mine' ? 'You are not enrolled in any course yet. See All courses.' : 'No courses yet.'}</p>
			{/each}
		</div>
	{/if}
</div>

<Modal bind:open={newOpen} title="New course" size="sm">
	<form id="new-course" onsubmit={create}>
		<label class="label" for="nc-title">Title</label>
		<input id="nc-title" class="input" required maxlength="200" bind:value={title} />
		<p class="mt-2 text-xs text-slate-500">It starts as a draft; publish it when it's ready.</p>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (newOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="new-course">Create</button>
	{/snippet}
</Modal>
