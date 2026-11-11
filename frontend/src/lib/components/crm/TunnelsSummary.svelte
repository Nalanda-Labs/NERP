<script lang="ts">
	// Pipelines & stages: the tunnels between pipelines (robots that copy or move a record into
	// another pipeline's stage), as arrows. They are set up in Automation.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import type { CrmCategory } from '#lib/crm.svelte.ts';

	let { categories }: { categories: CrmCategory[] } = $props();
	type Robot = { id: string; stage_id: string; action: string; params: { stage_id?: string; mode?: string }; is_active: boolean };
	let tunnels = $state<{ from: string; to: string; mode: string; active: boolean; category: string }[]>([]);
	const stageName = (id: string | undefined) => {
		for (const c of categories) {
			const s = c.stages.find((x) => x.id === id);
			if (s) return `${c.name}: ${s.name}`;
		}
		return '?';
	};
	$effect(() => {
		const cats = categories;
		untrack(async () => {
			const all = await Promise.all(cats.map((c) => api.get<{ robots: Robot[] }>(`/crm/automation?category_id=${c.id}`).then((r) => r.robots.map((x) => ({ ...x, category: c.id }))).catch(() => [])));
			tunnels = all.flat().filter((r) => r.action === 'tunnel').map((r) => ({ from: stageName(r.stage_id), to: stageName(r.params.stage_id), mode: r.params.mode ?? 'copy', active: r.is_active, category: r.category }));
		});
	});
</script>

{#if categories.length > 1}
	<section class="card" aria-labelledby="tunnels-h" data-tunnels>
		<div class="flex flex-wrap items-center gap-3 border-b border-slate-100 px-5 py-3">
			<div class="flex-1">
				<h2 id="tunnels-h" class="font-semibold text-slate-900">Tunnels between pipelines</h2>
				<p class="text-xs text-slate-500">When a record enters a stage, a tunnel copies or moves it into a stage of another pipeline. Add one in Automation, on the stage it starts from (“Tunnel to another pipeline”).</p>
			</div>
			<a class="btn btn-secondary" href="?tab=automation">Automation</a>
		</div>
		<ul class="divide-y divide-slate-50 text-sm">
			{#each tunnels as t, i (i)}
				<li class="flex flex-wrap items-center gap-2 px-5 py-2 {t.active ? '' : 'opacity-50'}" data-tunnel>
					<span class="font-medium text-slate-800">{t.from}</span>
					<span class="text-slate-400" aria-hidden="true">{t.mode === 'move' ? '⟹' : '⇉'}</span>
					<span class="font-medium text-slate-800">{t.to}</span>
					<span class="text-xs text-slate-500">({t.mode === 'move' ? 'moved' : 'copied'}{t.active ? '' : ', off'})</span>
				</li>
			{:else}
				<li class="px-5 py-3 text-slate-400">No tunnels yet.</li>
			{/each}
		</ul>
	</section>
{/if}
