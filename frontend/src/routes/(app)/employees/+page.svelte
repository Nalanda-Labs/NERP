<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api, downloadUrl, query } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM, type Department, type Page, type User } from '#lib/types.ts';

	const PAGE_SIZE = 25;
	let q = $state('');
	let departmentId = $state('');
	let status = $state('active');
	let offset = $state(0);
	let data = $state<Page<User> | null>(null);
	let departments = $state<Department[]>([]);
	let loading = $state(false);

	const canManage = $derived(session.can(PERM.USERS_MANAGE));

	async function load() {
		loading = true;
		try {
			data = await api.get<Page<User>>(
				'/users' + query({ q, department_id: departmentId, status, limit: PAGE_SIZE, offset })
			);
		} catch (e) {
			toasts.error(e);
		} finally {
			loading = false;
		}
	}

	let timer: ReturnType<typeof setTimeout>;
	function onSearch() {
		clearTimeout(timer);
		timer = setTimeout(() => {
			offset = 0;
			load();
		}, 250);
	}

	onMount(async () => {
		departments = await api.get<Department[]>('/departments');
		load();
	});

	// ----- add employee -----
	let addOpen = $state(false);
	let saving = $state(false);
	const blank = () => ({
		first_name: '',
		last_name: '',
		email: '',
		password: '',
		position: '',
		phone: '',
		invite_by_text: false,
		department_ids: [] as string[]
	});
	let form = $state(blank());
	// Invitations can go by text message too when SMS is set up.
	let canText = $state(false);
	$effect(() => {
		if (addOpen) api.get<{ enabled: boolean }>('/sms/available').then((r) => (canText = r.enabled)).catch(() => {});
	});
	let mode = $state<'invite' | 'password'>('invite');

	function generatePassword() {
		const chars = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789';
		const bytes = crypto.getRandomValues(new Uint8Array(14));
		form.password = Array.from(bytes, (b) => chars[b % chars.length]).join('');
	}

	async function create(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			const body =
				mode === 'invite' ? { ...form, password: undefined, send_invite: true, invite_by_text: form.invite_by_text && !!form.phone.trim() } : { ...form, send_invite: false, invite_by_text: false };
			const user = await api.post<User>('/users', body);
			toasts.success(mode === 'invite' ? `Invitation sent to ${user.email}` : `${user.full_name} was added`);
			addOpen = false;
			goto(`/employees/${user.id}`);
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
</script>

<svelte:head><title>Employees · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Employees</h1>
			<p class="text-sm text-slate-500">{data ? `${data.total} ${data.total === 1 ? 'person' : 'people'}` : ' '}</p>
		</div>
		<button class="btn btn-secondary ml-auto" onclick={() => downloadUrl('/api/users/export.csv', 'employees.csv').catch(toasts.error)}>Export CSV</button>
		<button class="btn btn-secondary" title="For Outlook and other address books; phones can sync the list with CardDAV (Profile → App passwords)" onclick={() => downloadUrl('/api/users/export.vcf', 'employees.vcf').catch(toasts.error)} data-export-vcf>vCards</button>
		{#if canManage || session.can(PERM.USERS_INVITE)}<a class="btn btn-secondary" href="/employees/invitations">Invitations</a> <a class="btn btn-secondary" href="/employees/guests">Guests</a>{/if}
		{#if canManage}
			<button
				class="btn btn-primary"
				onclick={() => {
					form = blank();
					mode = 'invite';
					addOpen = true;
				}}
			>
				<Icon name="plus" class="size-4" /> Add employee
			</button>
		{/if}
	</div>

	<div class="card flex flex-col gap-3 p-3 sm:flex-row">
		<div class="relative flex-1">
			<Icon name="search" class="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-slate-400" />
			<input class="input pl-9" placeholder="Search by name, email or position" bind:value={q} oninput={onSearch} />
		</div>
		<select class="input sm:w-56" bind:value={departmentId} onchange={() => ((offset = 0), load())}>
			<option value="">All departments</option>
			{#each departments as d (d.id)}
				<option value={d.id}>{d.name}</option>
			{/each}
		</select>
		{#if canManage}
			<select class="input sm:w-40" bind:value={status} onchange={() => ((offset = 0), load())}>
				<option value="active">Active</option>
				<option value="inactive">Deactivated</option>
				<option value="all">All</option>
			</select>
		{/if}
	</div>

	<div class="card overflow-hidden">
		<table class="min-w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500">
				<tr>
					<th class="px-5 py-3">Name</th>
					<th class="hidden px-5 py-3 md:table-cell">Department</th>
					<th class="hidden px-5 py-3 lg:table-cell">Contact</th>
					<th class="px-5 py-3 text-right">Status</th>
				</tr>
			</thead>
			<tbody class="divide-y divide-slate-100 {loading ? 'opacity-60' : ''}">
				{#each data?.items ?? [] as u (u.id)}
					<tr class="cursor-pointer hover:bg-slate-50" onclick={() => goto(`/employees/${u.id}`)}>
						<td class="px-5 py-3">
							<a href="/employees/{u.id}" class="flex items-center gap-3">
								<Avatar name={u.full_name} fileId={u.avatar_file_id} />
								<span class="min-w-0">
									<span class="block truncate font-medium text-slate-900">{u.full_name}</span>
									<span class="block truncate text-slate-500">{u.position ?? '—'}</span>
								</span>
							</a>
						</td>
						<td class="hidden px-5 py-3 text-slate-600 md:table-cell">
							{u.departments.map((d) => d.name).join(', ') || '—'}
						</td>
						<td class="hidden px-5 py-3 text-slate-600 lg:table-cell">
							<div>{u.email}</div>
							{#if u.phone || u.mobile}<div class="text-slate-400">{u.phone ?? u.mobile}</div>{/if}
						</td>
						<td class="px-5 py-3 text-right">
							{#if u.is_active && u.invite_pending}
								<span class="badge bg-amber-50 text-amber-700">Invited</span>
							{:else if u.is_active}
								<span class="badge bg-emerald-50 text-emerald-700">Active</span>
							{:else}
								<span class="badge bg-slate-100 text-slate-500">Deactivated</span>
							{/if}
						</td>
					</tr>
				{:else}
					<tr><td colspan="4" class="px-5 py-12 text-center text-slate-500">{loading ? 'Loading…' : 'No employees found.'}</td></tr>
				{/each}
			</tbody>
		</table>
		{#if data && data.total > PAGE_SIZE}
			<div class="flex items-center justify-between border-t border-slate-200 px-5 py-3 text-sm text-slate-500">
				<span>{offset + 1}–{Math.min(offset + PAGE_SIZE, data.total)} of {data.total}</span>
				<div class="flex gap-2">
					<button class="btn btn-secondary" disabled={offset === 0} onclick={() => ((offset -= PAGE_SIZE), load())}>Previous</button>
					<button class="btn btn-secondary" disabled={offset + PAGE_SIZE >= data.total} onclick={() => ((offset += PAGE_SIZE), load())}>Next</button>
				</div>
			</div>
		{/if}
	</div>
</div>

<Modal bind:open={addOpen} title="Add employee">
	<form id="add-employee" class="space-y-4" onsubmit={create}>
		<div class="grid grid-cols-2 gap-3">
			<div>
				<label class="label" for="fn">First name</label>
				<input id="fn" class="input" required bind:value={form.first_name} />
			</div>
			<div>
				<label class="label" for="ln">Last name</label>
				<input id="ln" class="input" required bind:value={form.last_name} />
			</div>
		</div>
		<div>
			<label class="label" for="em">Work email</label>
			<input id="em" class="input" type="email" required bind:value={form.email} />
		</div>
		<fieldset>
			<legend class="label">How will they sign in?</legend>
			<div class="grid grid-cols-2 gap-2">
				<label class="flex cursor-pointer items-start gap-2 rounded-lg border p-3 text-sm {mode === 'invite' ? 'border-brand-500 bg-brand-50' : 'border-slate-200'}">
					<input type="radio" class="mt-0.5 text-brand-600" value="invite" bind:group={mode} />
					<span><span class="block font-medium text-slate-800">Email an invitation</span><span class="text-xs text-slate-500">They choose their own password</span></span>
				</label>
				<label class="flex cursor-pointer items-start gap-2 rounded-lg border p-3 text-sm {mode === 'password' ? 'border-brand-500 bg-brand-50' : 'border-slate-200'}">
					<input type="radio" class="mt-0.5 text-brand-600" value="password" bind:group={mode} onchange={() => !form.password && generatePassword()} />
					<span><span class="block font-medium text-slate-800">Set a password</span><span class="text-xs text-slate-500">You share it with them</span></span>
				</label>
			</div>
		</fieldset>
		{#if mode === 'password'}
			<div>
				<label class="label" for="pw">Initial password</label>
				<div class="flex gap-2">
					<input id="pw" class="input font-mono" required minlength="8" bind:value={form.password} />
					<button type="button" class="btn btn-secondary shrink-0" onclick={generatePassword}>Generate</button>
				</div>
			</div>
		{/if}
		<div class="grid grid-cols-2 gap-3">
			<div>
				<label class="label" for="pos">Position</label>
				<input id="pos" class="input" bind:value={form.position} />
			</div>
			<div>
				<label class="label" for="ph">Phone</label>
				<input id="ph" class="input" type="tel" bind:value={form.phone} />
			</div>
		</div>
		{#if mode === 'invite' && canText}
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.invite_by_text} disabled={!form.phone.trim()} data-invite-by-text /> Also text the invitation to this phone</label>
		{/if}
		<div>
			<label class="label" for="dept">Departments</label>
			<select id="dept" class="input" multiple size={Math.min(6, Math.max(3, departments.length))} bind:value={form.department_ids}>
				{#each departments as d (d.id)}
					<option value={d.id}>{d.name}</option>
				{/each}
			</select>
			<p class="mt-1 text-xs text-slate-500">Hold Ctrl / ⌘ to select several.</p>
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (addOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="add-employee" disabled={saving}>
			{saving ? 'Saving…' : mode === 'invite' ? 'Send invitation' : 'Add employee'}
		</button>
	{/snippet}
</Modal>
