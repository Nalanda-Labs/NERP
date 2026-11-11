<script lang="ts">
	// Who may see or change my calendar: a level for everyone, and levels for departments and
	// single colleagues (the most specific one wins).
	import { api } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type Grant = { kind: 'user' | 'department'; id: string; level: string; name: string | null };
	let { open = $bindable(false) }: { open?: boolean } = $props();

	const LEVELS: [string, string][] = [['none', "Can't see it"], ['busy', 'Free/busy only'], ['read', 'See details'], ['edit', 'See and change']];
	let everyone = $state('read');
	let grants = $state<Grant[]>([]);
	let departments = $state<{ id: string; name: string }[]>([]);
	let dept = $state('');
	let busy = $state(false);

	$effect(() => {
		if (!open) return;
		api.get<{ everyone: string; grants: Grant[] }>('/calendar/access').then((r) => ((everyone = r.everyone), (grants = r.grants))).catch(toasts.error);
		api.get<{ id: string; name: string }[]>('/departments').then((d) => (departments = d)).catch(() => {});
	});
	function add(kind: 'user' | 'department', id: string, name: string) {
		if (!id || grants.some((g) => g.kind === kind && g.id === id)) return;
		grants = [...grants, { kind, id, name, level: 'read' }];
	}
	async function save() {
		busy = true;
		try {
			await api.put('/calendar/access', { everyone, grants: grants.map(({ kind, id, level }) => ({ kind, id, level })) });
			toasts.success('Sharing saved');
			open = false;
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
</script>

<Modal bind:open title="Share my calendar">
	<div class="space-y-4 text-sm">
		<div>
			<label class="label" for="cs-all">Everyone in the company</label>
			<select id="cs-all" class="input" bind:value={everyone}>{#each LEVELS.slice(0, 3) as [v, l] (v)}<option value={v}>{l}</option>{/each}</select>
			<p class="mt-1 text-xs text-slate-500">Private events always show as “Busy”, except to people who may change your calendar.</p>
		</div>
		<div class="space-y-2">
			<span class="label">Departments and people</span>
			{#if grants.length}
				<ul class="divide-y divide-slate-100 rounded-xl border border-slate-200" data-grants>
					{#each grants as g, i (g.kind + g.id)}
						<li class="flex items-center gap-2 px-3 py-2">
							<span class="flex-1 truncate">{g.kind === 'department' ? '🏢 ' : ''}{g.name ?? '…'}</span>
							<select class="input w-40 py-1" aria-label="Level for {g.name}" bind:value={grants[i].level}>{#each LEVELS as [v, l] (v)}<option value={v}>{l}</option>{/each}</select>
							<button class="px-1 text-slate-400 hover:text-red-600" aria-label="Remove {g.name}" onclick={() => (grants = grants.filter((_, j) => j !== i))}>×</button>
						</li>
					{/each}
				</ul>
			{/if}
			<div class="grid gap-2 sm:grid-cols-2">
				<UserPicker placeholder="Add a person…" onpick={(u) => u && add('user', u.id, u.full_name)} />
				<select class="input" aria-label="Add a department" bind:value={dept} onchange={() => { const d = departments.find((x) => x.id === dept); if (d) add('department', d.id, d.name); dept = ''; }}>
					<option value="">Add a department…</option>
					{#each departments as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
				</select>
			</div>
			<p class="text-xs text-slate-500">A person's own level wins over their department's, which wins over everyone's.</p>
		</div>
	</div>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" disabled={busy} onclick={save}>Save</button>
	{/snippet}
</Modal>
