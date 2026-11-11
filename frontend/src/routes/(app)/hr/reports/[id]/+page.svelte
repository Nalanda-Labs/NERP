<script lang="ts">
	// One work report, and its mark (for those who read it).
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Report = { id: string; user_id: string; user_name: string; period: string; period_start: string; period_end: string; body: string | null; plans: string | null; status: string; reviewer_ids: string[]; mark: string | null; review_comment: string | null; reviewed_by_name: string | null; figures: { tasks_done: { id: string; number: number; title: string }[]; worked_minutes: number; task_minutes: number; meetings: number } };
	let r = $state<Report | null>(null);
	let comment = $state('');
	onMount(async () => {
		try {
			r = await api.get<Report>(`/hr/reports/${page.params.id}`);
		} catch (e) {
			toasts.error(e);
		}
	});
	async function mark(m: string) {
		try {
			r = await api.post<Report>(`/hr/reports/${r!.id}/review`, { mark: m, comment: comment || null });
		} catch (e) {
			toasts.error(e);
		}
	}
	const MARK: Record<string, string> = { good: '👍 Good', bad: '👎 Needs work', none: 'Read' };
</script>

<svelte:head><title>Work report · NERP</title></svelte:head>

{#if r}
	<div class="max-w-3xl space-y-4">
		<a href="/hr/reports" class="text-sm text-brand-600 hover:underline">‹ Work reports</a>
		<h1 class="text-2xl font-semibold text-slate-900">{r.user_name} · {r.period === 'daily' ? new Date(r.period_start).toLocaleDateString() : `${new Date(r.period_start).toLocaleDateString()} – ${new Date(r.period_end).toLocaleDateString()}`}</h1>
		<section class="card space-y-3 p-5">
			<p class="text-sm text-slate-500">{r.figures.tasks_done.length} tasks done · {Math.round(r.figures.worked_minutes / 6) / 10} h worked · {Math.round(r.figures.task_minutes / 6) / 10} h on tasks · {r.figures.meetings} meetings</p>
			{#if r.figures.tasks_done.length}<ul class="text-sm">{#each r.figures.tasks_done as t (t.id)}<li>✓ <a class="hover:underline" href="/tasks/{t.id}">#{t.number} {t.title}</a></li>{/each}</ul>{/if}
			<div><h2 class="text-sm font-medium text-slate-700">Done</h2><p class="whitespace-pre-line text-slate-800" data-body>{r.body ?? ''}</p></div>
			{#if r.plans}<div><h2 class="text-sm font-medium text-slate-700">Next</h2><p class="whitespace-pre-line text-slate-800">{r.plans}</p></div>{/if}
		</section>
		{#if r.status === 'reviewed'}
			<p class="card p-4 text-sm" data-mark={r.mark}>{MARK[r.mark ?? 'none']} — {r.reviewed_by_name}{r.review_comment ? `: “${r.review_comment}”` : ''}</p>
		{:else if r.status === 'sent' && r.user_id !== session.user?.id}
			<section class="card space-y-2 p-4" aria-label="Mark">
				<textarea class="input" rows="2" aria-label="Comment" placeholder="A comment (optional)" bind:value={comment}></textarea>
				<div class="flex gap-2"><button class="btn btn-primary" onclick={() => mark('good')}>👍 Good</button><button class="btn btn-secondary" onclick={() => mark('bad')}>👎 Needs work</button><button class="btn btn-ghost" onclick={() => mark('none')}>Just read</button></div>
			</section>
		{/if}
	</div>
{/if}
