<script lang="ts">
	// A course's gradebook (people who manage training): everyone enrolled, their progress, each
	// test's best result and attempts, extra attempts, due dates and certificates.
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import { toasts } from '#lib/toast.svelte.ts';
	import { day, type Progress } from '#lib/training.ts';

	type Result = { test_id: string; attempts: number; best_percent: number | null; passed: boolean; last_attempt_id: string | null; extra: number; attempts_left: number | null };
	type Row = { user_id: string; name: string; avatar_file_id: string | null; source: string; due_on: string | null; enrolled_at: string; completed_at: string | null; overdue: boolean; progress: Progress; tests: Result[]; certificate: { id: string; number: number } | null };
	type Book = { course: { id: string; title: string }; tests: { id: string; title: string; attempt_limit: number; is_active: boolean }[]; people: Row[] };
	const id = $derived(page.params.id);
	let b = $state<Book | null>(null);
	let filter = $state<'all' | 'open' | 'overdue' | 'done'>('all');
	async function load() {
		try {
			b = await api.get<Book>(`/training/courses/${id}/gradebook`);
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		id;
		load();
	});
	async function moreAttempts(r: Row, t: Result) {
		try {
			await api.post(`/training/tests/${t.test_id}/extra-attempts`, { user_id: r.user_id, extra: t.extra + 1 });
			toasts.success(`${r.name} can try again`);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function unenrol(r: Row) {
		if (!confirm(`Take ${r.name} off the course? Their results stay if they come back.`)) return;
		await api.del(`/training/courses/${id}/enrolments/${r.user_id}`);
		await load();
	}
	const rows = $derived((b?.people ?? []).filter((r) => filter === 'all' || (filter === 'done' ? !!r.completed_at : filter === 'overdue' ? r.overdue : !r.completed_at)));
</script>

<svelte:head><title>Gradebook · {b?.course.title ?? ''} · NERP</title></svelte:head>

{#if b}
	<div class="space-y-4">
		<div><a href="/training/courses/{id}" class="text-sm text-slate-500 hover:underline">‹ {b.course.title}</a><h1 class="mt-1 text-2xl font-semibold text-slate-900">Gradebook</h1></div>
		<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist" style="width:fit-content">
			{#each [['all', 'Everyone'], ['open', 'In progress'], ['overdue', 'Overdue'], ['done', 'Completed']] as [k, label] (k)}
				<button role="tab" aria-selected={filter === k} class="rounded-full px-4 py-1.5 font-medium {filter === k ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (filter = k as typeof filter)}>{label}</button>
			{/each}
		</div>
		<div class="card overflow-x-auto">
			<table class="min-w-full text-sm">
				<thead class="border-b border-slate-100 text-left text-xs text-slate-500">
					<tr>
						<th class="px-4 py-3">Person</th><th class="px-3 py-3">Progress</th>
						{#each b.tests as t (t.id)}<th class="px-3 py-3">{t.title}{#if !t.is_active} (off){/if}</th>{/each}
						<th class="px-3 py-3">Due</th><th class="px-3 py-3">Certificate</th><th></th>
					</tr>
				</thead>
				<tbody class="divide-y divide-slate-50">
					{#each rows as r (r.user_id)}
						<tr data-person={r.name}>
							<td class="px-4 py-2.5"><div class="flex items-center gap-2"><Avatar name={r.name} fileId={r.avatar_file_id} size="sm" /><span><span class="block font-medium text-slate-900">{r.name}</span><span class="text-xs text-slate-400">{r.source === 'assigned' ? 'assigned' : 'enrolled'} {day(r.enrolled_at)}</span></span></div></td>
							<td class="px-3 py-2.5 tabular-nums">{r.completed_at ? '✓ ' + day(r.completed_at) : `${r.progress.percent} %`}<span class="block text-xs text-slate-400">{r.progress.lessons_done}/{r.progress.lessons} lessons</span></td>
							{#each r.tests as t (t.test_id)}
								<td class="px-3 py-2.5" data-result={t.test_id}>
									{#if t.attempts}
										<a class="tabular-nums hover:underline {t.passed ? 'text-emerald-700' : 'text-slate-700'}" href="/training/attempts/{t.last_attempt_id}">{t.best_percent ?? '…'} %{t.passed ? ' ✓' : ''}</a>
										<span class="block text-xs text-slate-400">{t.attempts} {t.attempts === 1 ? 'attempt' : 'attempts'}{t.attempts_left != null ? `, ${t.attempts_left} left` : ''}</span>
										{#if t.attempts_left === 0 && !t.passed}<button class="text-xs text-brand-600 hover:underline" onclick={() => moreAttempts(r, t)}>Allow one more</button>{/if}
									{:else}<span class="text-slate-300">—</span>{/if}
								</td>
							{/each}
							<td class="px-3 py-2.5 {r.overdue ? 'font-medium text-red-600' : 'text-slate-500'}">{r.due_on ? day(r.due_on) : ''}</td>
							<td class="px-3 py-2.5">{#if r.certificate}<a class="hover:underline" href="/training/certificates/{r.certificate.id}">No. {r.certificate.number}</a>{/if}</td>
							<td class="px-3 py-2.5"><button class="text-xs text-slate-400 hover:text-red-600" onclick={() => unenrol(r)}>Remove</button></td>
						</tr>
					{:else}
						<tr><td colspan={b.tests.length + 5} class="px-4 py-10 text-center text-slate-500">Nobody here.</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	</div>
{/if}
