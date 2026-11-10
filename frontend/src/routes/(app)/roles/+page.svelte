<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { PermissionInfo, Role } from '#lib/types.ts';

	let roles = $state<Role[]>([]);
	let catalog = $state<PermissionInfo[]>([]);
	let selectedId = $state<string | null>(null);
	let draft = $state<{ name: string; description: string; permissions: string[] }>({ name: '', description: '', permissions: [] });
	let saving = $state(false);

	const selected = $derived(roles.find((r) => r.id === selectedId) ?? null);
	const isAdminRole = $derived(selected?.code === 'admin');
	const groups = $derived(
		catalog.reduce<Record<string, PermissionInfo[]>>((acc, p) => {
			(acc[p.group] ??= []).push(p);
			return acc;
		}, {})
	);
	const dirty = $derived(
		!!selected &&
			(draft.name !== selected.name ||
				draft.description !== (selected.description ?? '') ||
				[...draft.permissions].sort().join() !== [...selected.permissions].sort().join())
	);

	async function load(keep?: string) {
		[roles, catalog] = await Promise.all([api.get<Role[]>('/roles'), api.get<PermissionInfo[]>('/permissions')]);
		pick(keep ?? selectedId ?? roles[0]?.id ?? null);
	}

	function pick(id: string | null) {
		selectedId = id;
		const r = roles.find((x) => x.id === id);
		if (r) draft = { name: r.name, description: r.description ?? '', permissions: [...r.permissions] };
	}

	function toggle(code: string, on: boolean) {
		draft.permissions = on ? [...draft.permissions, code] : draft.permissions.filter((p) => p !== code);
	}

	async function save() {
		if (!selected) return;
		saving = true;
		try {
			await api.patch(`/roles/${selected.id}`, {
				name: draft.name,
				description: draft.description || null,
				...(isAdminRole ? {} : { permissions: draft.permissions })
			});
			toasts.success('Role saved');
			await load(selected.id);
			// Our own permissions may have changed.
			await session.reloadMe();
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}

	async function remove() {
		if (!selected || !confirm(`Delete the role "${selected.name}"? ${selected.user_count} employees will lose it.`)) return;
		try {
			await api.del(`/roles/${selected.id}`);
			toasts.success('Role deleted');
			selectedId = null;
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}

	// ----- create -----
	let createOpen = $state(false);
	let newRole = $state({ name: '', code: '', description: '' });
	let codeTouched = $state(false);

	$effect(() => {
		if (!codeTouched) {
			newRole.code = newRole.name.toLowerCase().trim().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '').replace(/^[^a-z]+/, '').slice(0, 50);
		}
	});

	async function create(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			const r = await api.post<Role>('/roles', { ...newRole, permissions: [] });
			toasts.success('Role created — now choose its permissions');
			createOpen = false;
			await load(r.id);
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}

	onMount(() => load());
</script>

<svelte:head><title>Roles & permissions · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Roles & permissions</h1>
			<p class="text-sm text-slate-500">Every employee has the Employee role; give extra roles for more access.</p>
		</div>
		<button
			class="btn btn-primary"
			onclick={() => {
				newRole = { name: '', code: '', description: '' };
				codeTouched = false;
				createOpen = true;
			}}
		>
			<Icon name="plus" class="size-4" /> New role
		</button>
	</div>

	<div class="grid gap-6 lg:grid-cols-[300px_1fr]">
		<ul class="card h-fit divide-y divide-slate-100 overflow-hidden">
			{#each roles as r (r.id)}
				<li>
					<button
						class="flex w-full items-center justify-between gap-2 px-4 py-3 text-left {r.id === selectedId ? 'bg-brand-50' : 'hover:bg-slate-50'}"
						onclick={() => pick(r.id)}
					>
						<span class="min-w-0">
							<span class="block truncate text-sm font-medium {r.id === selectedId ? 'text-brand-700' : 'text-slate-800'}">{r.name}</span>
							<span class="block text-xs text-slate-500">{r.user_count} {r.user_count === 1 ? 'person' : 'people'}</span>
						</span>
						{#if r.is_system}<span class="badge bg-slate-100 text-slate-500">Built-in</span>{/if}
					</button>
				</li>
			{/each}
		</ul>

		{#if selected}
			<div class="card">
				<div class="space-y-4 border-b border-slate-100 p-5">
					<div class="grid gap-4 sm:grid-cols-2">
						<div><label class="label" for="r-name">Name</label><input id="r-name" class="input" bind:value={draft.name} /></div>
						<div>
							<label class="label" for="r-code">Code</label>
							<input id="r-code" class="input font-mono" value={selected.code} disabled />
						</div>
					</div>
					<div><label class="label" for="r-desc">Description</label><input id="r-desc" class="input" bind:value={draft.description} /></div>
					<p class="text-sm text-slate-500">
						{selected.user_count} active {selected.user_count === 1 ? 'employee has' : 'employees have'} this role.
						Assign roles from an employee's profile.
					</p>
				</div>

				<div class="space-y-6 p-5">
					{#if isAdminRole}
						<p class="rounded-lg bg-amber-50 px-4 py-3 text-sm text-amber-800">
							Administrators always have every permission, including those added by future modules.
						</p>
					{/if}
					{#each Object.entries(groups) as [group, perms] (group)}
						<div>
							<h3 class="mb-2 text-sm font-medium text-slate-600">{group}</h3>
							<div class="grid gap-2 sm:grid-cols-2">
								{#each perms as p (p.code)}
									<label class="flex items-start gap-3 rounded-lg border border-slate-200 p-3 {isAdminRole ? 'opacity-70' : 'hover:bg-slate-50'}">
										<input
											type="checkbox"
											class="mt-0.5 rounded border-slate-300 text-brand-600 focus:ring-brand-500"
											disabled={isAdminRole}
											checked={isAdminRole || draft.permissions.includes(p.code)}
											onchange={(e) => toggle(p.code, e.currentTarget.checked)}
										/>
										<span>
											<span class="block text-sm font-medium text-slate-800">{p.name}</span>
											<span class="block text-xs text-slate-500">{p.description}</span>
										</span>
									</label>
								{/each}
							</div>
						</div>
					{/each}
				</div>

				<div class="flex items-center justify-between gap-2 border-t border-slate-100 px-5 py-3">
					{#if !selected.is_system}
						<button class="btn btn-ghost text-red-600 hover:bg-red-50" onclick={remove}><Icon name="trash" class="size-4" /> Delete role</button>
					{:else}
						<span></span>
					{/if}
					<div class="flex gap-2">
						<button class="btn btn-secondary" disabled={!dirty} onclick={() => pick(selected.id)}>Discard</button>
						<button class="btn btn-primary" disabled={!dirty || saving} onclick={save}>Save changes</button>
					</div>
				</div>
			</div>
		{/if}
	</div>
</div>

<Modal bind:open={createOpen} title="New role" size="sm">
	<form id="new-role" class="space-y-4" onsubmit={create}>
		<div><label class="label" for="n-name">Name</label><input id="n-name" class="input" required bind:value={newRole.name} placeholder="e.g. HR manager" /></div>
		<div>
			<label class="label" for="n-code">Code</label>
			<input id="n-code" class="input font-mono" required pattern="[a-z][a-z0-9_\-]{'{1,49}'}" bind:value={newRole.code} oninput={() => (codeTouched = true)} />
			<p class="mt-1 text-xs text-slate-500">A permanent identifier: lowercase letters, digits, - and _.</p>
		</div>
		<div><label class="label" for="n-desc">Description</label><input id="n-desc" class="input" bind:value={newRole.description} /></div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (createOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="new-role" disabled={saving}>Create</button>
	{/snippet}
</Modal>
