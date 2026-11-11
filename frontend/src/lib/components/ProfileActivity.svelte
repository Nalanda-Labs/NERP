<script lang="ts">
	// On a profile: the person's appreciation badges, and tabs with their open tasks, their
	// week in the calendar (as they share it), and their documents to sign (for themselves,
	// their managers and HR).
	import { api, query } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';

	let { userId }: { userId: string } = $props();
	type Badges = { counts: { code: string; label: string; count: number }[]; recent: { post_id: string; label: string; from: string | null; body: string; at: string }[] };
	const ICON: Record<string, string> = { thanks: '🙏', team_player: '🤝', great_idea: '💡', problem_solver: '🧩', leader: '⭐', above_beyond: '🚀' };
	let badges = $state<Badges | null>(null);
	let tab = $state<'tasks' | 'week' | 'documents' | 'files'>('tasks');
	let files = $state<{ id: string; name: string; kind: 'file' | 'folder'; updated_at?: string }[] | null>(null);
	let tasks = $state<{ id: string; number: number; title: string; deadline: string | null; status: string }[] | null>(null);
	let week = $state<{ id: string; title: string; start: string; all_day: boolean; busy: boolean }[] | null>(null);
	let docs = $state<{ id: string; title: string; status: string; created_at: string }[] | null>(null);
	let noDocs = $state(false);

	$effect(() => {
		api.get<Badges>(`/users/${userId}/badges`).then((b) => (badges = b)).catch(() => {});
	});
	$effect(() => {
		if (tab === 'tasks') api.get<{ items: typeof tasks }>('/tasks' + query({ responsible_id: userId, status: 'new,in_progress,review', limit: 50, sort: 'deadline' })).then((r) => (tasks = r.items)).catch(() => (tasks = []));
		if (tab === 'week') {
			const from = new Date(); from.setHours(0, 0, 0, 0);
			const to = new Date(+from + 7 * 86400e3);
			api.get<{ items: (NonNullable<typeof week>[number] & { user_id: string; kind: string })[] }>('/calendar' + query({ from: from.toISOString(), to: to.toISOString(), users: userId }))
				.then((r) => (week = r.items.filter((x) => x.user_id === userId && x.kind === 'event')))
				.catch(() => (week = []));
		}
		if (tab === 'files') api.get<{ items: typeof files }>('/drive' + query({ space: 'shared', owner: userId })).then((r) => (files = r.items)).catch(() => (files = []));
		if (tab === 'documents') api.get<{ items: typeof docs }>('/hr/docs' + query({ scope: 'employee', employee_id: userId })).then((r) => ((docs = r.items), (noDocs = false))).catch(() => (noDocs = true));
	});
	const STATUS: Record<string, string> = { approval: 'Being approved', signing: 'Being signed', signed: 'Signed', declined: 'Declined', revoked: 'Not valid' };
</script>

{#if badges?.counts.length}
	<section class="card" data-badges>
		<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">Appreciation</h2>
		<div class="flex flex-wrap gap-2 px-5 pt-4">{#each badges.counts as c (c.code)}<span class="rounded-full bg-amber-50 px-3 py-1 text-sm text-amber-900">{ICON[c.code]} {c.label}{c.count > 1 ? ` × ${c.count}` : ''}</span>{/each}</div>
		<ul class="space-y-1 px-5 py-3 text-sm text-slate-600">{#each badges.recent.slice(0, 3) as r (r.post_id)}<li><a class="hover:underline" href="/feed?post={r.post_id}">{r.label} from {r.from ?? 'someone'}: “{r.body}”</a></li>{/each}</ul>
	</section>
{/if}

<section class="card">
	<div class="flex gap-1 border-b border-slate-100 px-3 pt-2" role="tablist">
{#each [['tasks', 'Tasks'], ['week', 'This week'], ['files', 'Files'], ['documents', 'Documents']] as [k, l] (k)}
			<button role="tab" aria-selected={tab === k} class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {tab === k ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500'}" onclick={() => (tab = k as typeof tab)}>{l}</button>
		{/each}
	</div>
	<div class="p-4 text-sm" data-profile-tab={tab}>
		{#if tab === 'tasks'}
			{#each tasks ?? [] as t (t.id)}<p><a class="hover:underline" href="/tasks/{t.id}">#{t.number} {t.title}</a>{#if t.deadline}<span class="text-slate-500"> · due {formatDate(t.deadline)}</span>{/if}</p>{:else}<p class="text-slate-400">No open tasks you can see.</p>{/each}
		{:else if tab === 'week'}
			{#each week ?? [] as e (e.id + e.start)}<p><span class="text-slate-500">{new Date(e.start).toLocaleString([], { weekday: 'short', hour: e.all_day ? undefined : '2-digit', minute: e.all_day ? undefined : '2-digit' })}</span> {e.busy ? 'Busy' : e.title}</p>{:else}<p class="text-slate-400">Nothing this week (or the calendar isn't shared with you).</p>{/each}
		{:else if tab === 'files'}
			{#each files ?? [] as f (f.id)}<p><a class="hover:underline" href={f.kind === 'folder' ? `/drive?folder=${f.id}` : `/drive?space=shared&file=${f.id}`}>{f.kind === 'folder' ? '📁' : '📄'} {f.name}</a></p>{:else}<p class="text-slate-400">Nothing of theirs is shared with you.</p>{/each}
		{:else if noDocs}
			<p class="text-slate-400">Only the person, their managers and HR see their documents.</p>
		{:else}
			{#each docs ?? [] as d (d.id)}<p><a class="hover:underline" href="/hr/documents/{d.id}">{d.title}</a> <span class="text-slate-500">· {STATUS[d.status]} · {formatDate(d.created_at)}</span></p>{:else}<p class="text-slate-400">No documents.</p>{/each}
		{/if}
	</div>
</section>
