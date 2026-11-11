<script lang="ts">
	// My day: the work day, today's shift and check-in, tasks due, events, my plan for the
	// day, and the day's report.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Day = {
		day: string;
		workday: { state: 'none' | 'working' | 'paused' | 'ended'; worked_minutes?: number; started_at?: string };
		shift: { name: string; start_min: number; end_min: number; color: string } | null;
		checkin: { status: string; location: string | null } | null;
		tasks: { id: string; number: number; title: string; deadline: string | null; overdue: boolean }[];
		events: { id: string; title: string; start: string; end: string; all_day: boolean; location: string | null }[];
		plan: { id: string; title: string; task_id: string | null; task_number: number | null; done: boolean }[];
	};
	type Report = { id: string; status: string; body: string | null; plans: string | null; mark: string | null; review_comment: string | null; figures: { tasks_done: { number: number; title: string }[]; worked_minutes: number; task_minutes: number; meetings: number } };
	let d = $state<Day | null>(null);
	let item = $state('');
	let report = $state<Report | null>(null);
	let readers = $state<string[]>([]);
	let text = $state({ body: '', plans: '' });

	const hm = (m: number) => `${String(Math.floor(m / 60) % 24).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
	const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	async function load() {
		try {
			d = await api.get<Day>('/hr/my-day');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function loadReport() {
		const r = await api.get<{ report: Report; readers: string[] }>('/hr/reports/current?period=daily');
		report = r.report;
		readers = r.readers;
		text = { body: report.body ?? '', plans: report.plans ?? '' };
	}
	onMount(() => {
		load();
		loadReport().catch(() => {});
	});
	async function workday(action: string) {
		try {
			await api.post(`/hr/workday/${action}`, {});
			window.dispatchEvent(new Event('nerp:workday'));
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function addItem(e: SubmitEvent) {
		e.preventDefault();
		if (!item.trim()) return;
		d = await api.post<Day>('/hr/my-day/plan', { title: item });
		item = '';
	}
	const plan = async (task_id: string) => (d = await api.post<Day>('/hr/my-day/plan', { task_id }));
	const tick = async (id: string, done: boolean) => (d = await api.patch<Day>(`/hr/my-day/plan/${id}`, { done }));
	const drop = async (id: string) => (d = await api.del<Day>(`/hr/my-day/plan/${id}`));
	async function saveReport(sendIt: boolean) {
		if (!report) return;
		try {
			report = await api.put<Report>(`/hr/reports/${report.id}`, text);
			if (sendIt) {
				report = await api.post<Report>(`/hr/reports/${report.id}/send`, {});
				toasts.success('Report sent');
			} else toasts.success('Saved');
		} catch (e) {
			toasts.error(e);
		}
	}
	function fillFromPlan() {
		const done = d?.plan.filter((p) => p.done).map((p) => `- ${p.title}`) ?? [];
		const open = d?.plan.filter((p) => !p.done).map((p) => `- ${p.title}`) ?? [];
		text = { body: [text.body, ...done].filter(Boolean).join('\n'), plans: [text.plans, ...open].filter(Boolean).join('\n') };
	}
</script>

<svelte:head><title>My day · NERP</title></svelte:head>

{#if d}
	<div class="space-y-4">
		<div class="flex flex-wrap items-center gap-3">
			<h1 class="text-2xl font-semibold text-slate-900">My day</h1>
			<span class="text-slate-500">{new Date(d.day).toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' })}</span>
			<div class="ml-auto flex items-center gap-2" data-day-state={d.workday.state}>
				{#if d.workday.state === 'none'}<button class="btn btn-primary" onclick={() => workday('start')}>Start the day</button>
				{:else if d.workday.state === 'working'}<button class="btn btn-secondary" onclick={() => workday('pause')}>Pause</button><button class="btn btn-secondary" onclick={() => workday('end')}>End the day</button>
				{:else if d.workday.state === 'paused'}<button class="btn btn-primary" onclick={() => workday('resume')}>Resume</button>
				{:else}<span class="text-sm text-slate-500">Day ended · {Math.floor((d.workday.worked_minutes ?? 0) / 60)} h {(d.workday.worked_minutes ?? 0) % 60} min</span>{/if}
			</div>
		</div>
		<div class="flex flex-wrap gap-3 text-sm">
			{#if d.shift}<span class="rounded-full px-3 py-1 text-white" style="background:{d.shift.color}" data-shift>{d.shift.name} · {hm(d.shift.start_min)}–{hm(d.shift.end_min)}</span>{/if}
			{#if d.checkin}<span class="rounded-full bg-slate-100 px-3 py-1">Checked in: {d.checkin.status === 'working' ? (d.checkin.location ?? 'working') : 'not working'}</span>{:else}<a class="rounded-full bg-slate-100 px-3 py-1 text-brand-700" href="/hr/checkin">Check in</a>{/if}
		</div>
		<div class="grid gap-4 lg:grid-cols-3">
			<section class="card p-4 lg:col-span-2" aria-label="My plan">
				<h2 class="mb-2 font-medium text-slate-900">My plan for today</h2>
				<ul class="space-y-1" data-plan>
					{#each d.plan as p (p.id)}
						<li class="flex items-center gap-2">
							<input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={p.done} aria-label="Done: {p.title}" onchange={(e) => tick(p.id, e.currentTarget.checked)} />
							<span class="flex-1 {p.done ? 'text-slate-400 line-through' : 'text-slate-800'}">{#if p.task_id}<a class="hover:underline" href="/tasks/{p.task_id}">#{p.task_number} {p.title}</a>{:else}{p.title}{/if}</span>
							<button class="text-slate-300 hover:text-red-600" aria-label="Remove {p.title}" onclick={() => drop(p.id)}>×</button>
						</li>
					{:else}<li class="text-sm text-slate-400">Nothing planned yet.</li>{/each}
				</ul>
				<form class="mt-3 flex gap-2" onsubmit={addItem}><input class="input" aria-label="Add to my plan" placeholder="Add something to do today…" maxlength="300" bind:value={item} /><button class="btn btn-secondary">Add</button></form>
			</section>
			<section class="card p-4" aria-label="Today's events">
				<h2 class="mb-2 font-medium text-slate-900">Today</h2>
				<ul class="space-y-1.5 text-sm">
					{#each d.events as e (e.id + e.start)}<li><span class="text-slate-500">{e.all_day ? 'All day' : time(e.start)}</span> <a class="text-slate-800 hover:underline" href="/calendar?event={e.id}">{e.title}</a></li>{:else}<li class="text-slate-400">No meetings.</li>{/each}
				</ul>
				<h2 class="mt-4 mb-2 font-medium text-slate-900">Tasks due</h2>
				<ul class="space-y-1.5 text-sm" data-due>
					{#each d.tasks as t (t.id)}
						<li class="flex items-center gap-2"><a class="flex-1 hover:underline {t.overdue ? 'text-red-700' : 'text-slate-800'}" href="/tasks/{t.id}">#{t.number} {t.title}</a>{#if !d.plan.some((p) => p.task_id === t.id)}<button class="text-xs text-brand-600 hover:underline" onclick={() => plan(t.id)}>Plan</button>{/if}</li>
					{:else}<li class="text-slate-400">Nothing due.</li>{/each}
				</ul>
			</section>
		</div>
		{#if report}
			<section class="card space-y-3 p-4" aria-label="Today's report" data-report={report.status}>
				<div class="flex items-center gap-2">
					<h2 class="font-medium text-slate-900">Today's report</h2>
					<span class="text-xs text-slate-500">{readers.length ? `for ${readers.join(', ')}` : 'nobody reads your reports yet'}</span>
					<span class="ml-auto text-xs text-slate-500">{report.figures.tasks_done.length} tasks done · {Math.round(report.figures.worked_minutes / 6) / 10} h worked · {report.figures.meetings} meetings</span>
				</div>
				{#if report.status === 'draft'}
					<textarea class="input" rows="3" aria-label="What I did" placeholder="What I did today" bind:value={text.body}></textarea>
					<textarea class="input" rows="2" aria-label="Plans" placeholder="What comes next" bind:value={text.plans}></textarea>
					<div class="flex gap-2"><button class="btn btn-ghost" onclick={fillFromPlan}>Fill in from my plan</button><button class="btn btn-secondary ml-auto" onclick={() => saveReport(false)}>Save</button><button class="btn btn-primary" disabled={!text.body.trim() || !readers.length} onclick={() => saveReport(true)}>Send</button></div>
				{:else}
					<p class="text-sm whitespace-pre-line text-slate-700">{report.body}</p>
					<p class="text-sm text-slate-500">{report.status === 'reviewed' ? `Marked ${report.mark}${report.review_comment ? `: “${report.review_comment}”` : ''}` : 'Sent'}</p>
				{/if}
			</section>
		{/if}
	</div>
{/if}
