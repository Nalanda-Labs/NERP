<script lang="ts">
	// On a profile: the cross-functional teams the person is in.
	import { api } from '#lib/api.ts';

	let { userId }: { userId: string } = $props();
	let teams = $state<{ id: string; name: string; color: string | null; role: string | null; lead: boolean }[]>([]);
	$effect(() => {
		api.get<{ items: typeof teams }>(`/users/${userId}/teams`).then((r) => (teams = r.items)).catch(() => (teams = []));
	});
</script>

{#if teams.length}
	<section class="card" aria-label="Teams">
		<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">Teams</h2>
		<ul class="flex flex-wrap gap-2 p-5 text-sm">
			{#each teams as t (t.id)}
				<li><a class="badge flex items-center gap-1.5 bg-slate-100 text-slate-700 hover:bg-slate-200" href="/teams" data-profile-team={t.name}><span class="size-2 rounded-full" style="background: {t.color ?? '#94a3b8'}"></span>{t.name}{t.lead ? ' · lead' : t.role ? ` · ${t.role}` : ''}</a></li>
			{/each}
		</ul>
	</section>
{/if}
