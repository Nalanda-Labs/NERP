<script lang="ts">
	// Search box that finds tasks by title or #number.
	import { api, query } from '#lib/api.ts';
	import { STATUS, type TaskSummary } from '#lib/tasks.ts';
	import type { Page } from '#lib/types.ts';

	let {
		exclude = [],
		placeholder = 'Search tasks or #number…',
		onpick
	}: { exclude?: string[]; placeholder?: string; onpick: (t: TaskSummary) => void } = $props();

	let term = $state('');
	let results = $state<TaskSummary[]>([]);
	let open = $state(false);
	let timer: ReturnType<typeof setTimeout>;

	function search() {
		clearTimeout(timer);
		timer = setTimeout(async () => {
			const p = await api.get<Page<TaskSummary>>('/tasks' + query({ q: term, sort: 'activity', limit: 8 }));
			results = p.items.filter((t) => !exclude.includes(t.id));
			open = true;
		}, 200);
	}
</script>

<div class="relative">
	<input class="input" {placeholder} bind:value={term} oninput={search} onfocus={search} onblur={() => setTimeout(() => (open = false), 150)} />
	{#if open && results.length}
		<ul class="absolute z-20 mt-1 max-h-64 w-full overflow-auto rounded-xl bg-white py-1 shadow-g2">
			{#each results as t (t.id)}
				<li>
					<button
						type="button"
						class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-slate-50"
						onmousedown={() => {
							onpick(t);
							term = '';
							open = false;
						}}
					>
						<span class="size-2 shrink-0 rounded-full {STATUS[t.status].dot}"></span>
						<span class="shrink-0 text-xs text-slate-400">#{t.number}</span>
						<span class="truncate text-slate-800">{t.title}</span>
					</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>
