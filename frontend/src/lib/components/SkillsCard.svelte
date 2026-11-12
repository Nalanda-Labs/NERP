<script lang="ts">
	// What an employee is good at (languages, products, regions), used to assign work.
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { userId, canEdit }: { userId: string; canEdit: boolean } = $props();
	let skills = $state<string[]>([]);
	let adding = $state('');
	$effect(() => {
		api.get<{ skills: string[] }>(`/users/${userId}/skills`).then((r) => (skills = r.skills)).catch(() => (skills = []));
	});
	async function save(next: string[]) {
		try {
			skills = (await api.put<{ skills: string[] }>(`/users/${userId}/skills`, { skills: next })).skills;
		} catch (e) {
			toasts.error(e);
		}
	}
	function add(e: SubmitEvent) {
		e.preventDefault();
		const v = adding.trim();
		if (!v) return;
		adding = '';
		save([...skills, v]);
	}
</script>

{#if skills.length || canEdit}
	<section class="card" aria-label="Skills">
		<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">Skills</h2>
		<div class="space-y-3 p-5 text-sm">
			<div class="flex flex-wrap gap-1.5" data-skills>
				{#each skills as s (s)}
					<span class="badge flex items-center gap-1 bg-slate-100 text-slate-700" data-skill={s}>{s}{#if canEdit}<button type="button" class="text-slate-400 hover:text-red-600" aria-label="Remove {s}" onclick={() => save(skills.filter((x) => x !== s))}>×</button>{/if}</span>
				{:else}<span class="text-slate-500">None yet. Skills (languages, products, regions) help give work to the right person.</span>{/each}
			</div>
			{#if canEdit}
				<form class="flex gap-2" onsubmit={add}><input class="input w-56" placeholder="e.g. German, Solar, North" maxlength="60" bind:value={adding} aria-label="New skill" /><button class="btn btn-secondary">Add</button></form>
			{/if}
		</div>
	</section>
{/if}
