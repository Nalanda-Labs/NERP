<script lang="ts">
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import ChatCallButtons from '#lib/components/ChatCallButtons.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import ProfileActivity from '#lib/components/ProfileActivity.svelte';
	import SkillsCard from '#lib/components/SkillsCard.svelte';
	import TeamsCard from '#lib/components/TeamsCard.svelte';
	import SessionsCard from '#lib/components/SessionsCard.svelte';
	import { formatDate, formatDateTime } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM, type Department, type Role, type User } from '#lib/types.ts';

	let user = $state<User | null>(null);
	let notFound = $state(false);
	let departments = $state<Department[]>([]);
	let roles = $state<Role[]>([]);

	const id = $derived(page.params.id!);
	const isMe = $derived(session.user?.id === id);
	const canManage = $derived(session.can(PERM.USERS_MANAGE));
	const canRoles = $derived(session.can(PERM.ROLES_MANAGE));

	async function load() {
		try {
			user = await api.get<User>(`/users/${id}`);
		} catch {
			notFound = true;
		}
	}

	$effect(() => {
		id;
		user = null;
		notFound = false;
		load();
	});

	// ----- edit -----
	let editOpen = $state(false);
	let saving = $state(false);
	let edit = $state<Record<string, unknown>>({});

	async function openEdit() {
		departments = await api.get<Department[]>('/departments');
		const u = user!;
		edit = {
			first_name: u.first_name,
			last_name: u.last_name,
			middle_name: u.middle_name ?? '',
			email: u.email,
			position: u.position ?? '',
			phone: u.phone ?? '',
			mobile: u.mobile ?? '',
			birthday: u.birthday ?? '',
			department_ids: u.departments.map((d) => d.id)
		};
		editOpen = true;
	}

	async function saveEdit(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			user = await api.patch<User>(`/users/${id}`, { ...edit, birthday: edit.birthday || null });
			toasts.success('Profile updated');
			editOpen = false;
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}

	// ----- roles -----
	let rolesOpen = $state(false);
	let selectedRoles = $state<string[]>([]);

	async function openRoles() {
		roles = await api.get<Role[]>('/roles');
		selectedRoles = user!.roles.map((r) => r.id);
		rolesOpen = true;
	}

	async function saveRoles() {
		saving = true;
		try {
			user = await api.put<User>(`/users/${id}/roles`, { role_ids: selectedRoles });
			toasts.success('Roles updated');
			rolesOpen = false;
			if (isMe) await session.reloadMe();
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}

	// ----- password reset -----
	let pwOpen = $state(false);
	let newPassword = $state('');

	async function resetPassword(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			await api.post(`/users/${id}/password`, { password: newPassword });
			toasts.success('Password reset. The employee has been signed out everywhere.');
			pwOpen = false;
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}

	async function resendInvite() {
		try {
			await api.post(`/users/${id}/invite`);
			toasts.success(`A new invitation was sent to ${user!.email}`);
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}

	async function resetTwoFactor() {
		if (!confirm(`Turn off two-factor authentication for ${user!.full_name}? Use this if they lost their phone and recovery codes.`)) return;
		try {
			await api.post(`/users/${id}/2fa/reset`);
			toasts.success('Two-factor authentication turned off');
			await load();
		} catch (err) {
			toasts.error(err);
		}
	}

	async function setActive(active: boolean) {
		if (!active && !confirm(`Deactivate ${user!.full_name}? They will be signed out and unable to log in.`)) return;
		try {
			user = await api.patch<User>(`/users/${id}`, { is_active: active });
			toasts.success(active ? 'Employee reactivated' : 'Employee deactivated');
		} catch (err) {
			toasts.error(err);
		}
	}
	const directChat = async () => (await api.post<{ id: string }>(`/im/direct/${user!.id}`, {})).id;
</script>

<svelte:head><title>{user?.full_name ?? 'Employee'} · NERP</title></svelte:head>

<a href="/employees" class="mb-4 inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800">
	<Icon name="chevronRight" class="size-4 rotate-180" /> Employees
</a>

{#if notFound}
	<div class="card p-10 text-center text-slate-500">This employee could not be found.</div>
{:else if user}
	<div class="grid gap-6 lg:grid-cols-3">
		<div class="card p-6 text-center lg:col-span-1">
			<div class="flex justify-center"><Avatar name={user.full_name} fileId={user.avatar_file_id} size="xl" /></div>
			<h1 class="mt-4 text-xl font-semibold text-slate-900">{user.full_name}</h1>
			{#if user.middle_name}<p class="text-sm text-slate-400">{user.first_name} {user.middle_name} {user.last_name}</p>{/if}
			<p class="text-slate-500">{user.position ?? '—'}</p>
			<p class="mt-1 text-xs {user.online ? 'text-emerald-700' : 'text-slate-400'}" data-presence>{user.online ? '● Online' : user.last_seen_at ? `Last seen ${formatDateTime(user.last_seen_at)}` : 'Not seen yet'}</p>
			<div class="mt-3 flex flex-wrap justify-center gap-1.5">
				{#if !user.is_active}<span class="badge bg-red-50 text-red-700">Deactivated</span>{/if}
				{#if user.invite_pending}<span class="badge bg-amber-50 text-amber-700">Invitation pending</span>{/if}
				{#if user.two_factor_enabled}<span class="badge bg-emerald-50 text-emerald-700">2FA on</span>{/if}
				{#each user.roles.filter((r) => r.code !== 'employee') as r (r.id)}
					<span class="badge bg-brand-50 text-brand-700">{r.name}</span>
				{/each}
			</div>
			<div class="mt-6 flex flex-col gap-2">
				{#if isMe}
					<a href="/profile" class="btn btn-secondary"><Icon name="edit" class="size-4" /> Edit my profile</a>
				{:else if user.is_active}
					<div class="grid grid-cols-2 gap-2"><ChatCallButtons chat={directChat} chatLabel="Message" primary /></div>
				{/if}
				{#if canManage}
					<button class="btn btn-secondary" onclick={openEdit}><Icon name="edit" class="size-4" /> Edit details</button>
				{/if}
				{#if canRoles}
					<button class="btn btn-secondary" onclick={openRoles}><Icon name="shield" class="size-4" /> Manage roles</button>
				{/if}
				{#if canManage && !isMe}
					{#if user.invite_pending && user.is_active}
						<button class="btn btn-secondary" onclick={resendInvite}><Icon name="send" class="size-4" /> Resend invitation</button>
					{/if}
					<button class="btn btn-secondary" onclick={() => ((newPassword = ''), (pwOpen = true))}>
						<Icon name="key" class="size-4" /> Reset password
					</button>
					{#if user.two_factor_enabled}
						<button class="btn btn-secondary" onclick={resetTwoFactor}><Icon name="smartphone" class="size-4" /> Turn off 2FA</button>
					{/if}
					{#if user.is_active}
						<button class="btn btn-ghost text-red-600 hover:bg-red-50 hover:text-red-700" onclick={() => setActive(false)}>Deactivate</button>
					{:else}
						<button class="btn btn-primary" onclick={() => setActive(true)}>Reactivate</button>
					{/if}
				{/if}
			</div>
		</div>

		<div class="space-y-6 lg:col-span-2">
			<section class="card">
				<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">Contact</h2>
				<dl class="grid gap-x-6 gap-y-4 p-5 text-sm sm:grid-cols-2">
					<div>
						<dt class="text-slate-500">Email</dt>
						<dd class="mt-0.5"><a class="text-brand-600 hover:underline" href="mailto:{user.email}">{user.email}</a></dd>
					</div>
					<div><dt class="text-slate-500">Work phone</dt><dd class="mt-0.5 text-slate-800">{user.phone ?? '—'}</dd></div>
					<div><dt class="text-slate-500">Mobile</dt><dd class="mt-0.5 text-slate-800">{user.mobile ?? '—'}</dd></div>
					<div><dt class="text-slate-500">Time zone</dt><dd class="mt-0.5 text-slate-800">{user.timezone}</dd></div>
				</dl>
			</section>

			<section class="card">
				<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">Work</h2>
				<dl class="grid gap-x-6 gap-y-4 p-5 text-sm sm:grid-cols-2">
					<div>
						<dt class="text-slate-500">Departments</dt>
						<dd class="mt-0.5 text-slate-800">
							{#each user.departments as d, i (d.id)}{i ? ', ' : ''}<a class="text-brand-600 hover:underline" href="/structure?d={d.id}">{d.name}</a>{:else}—{/each}
						</dd>
					</div>
					<div><dt class="text-slate-500">Position</dt><dd class="mt-0.5 text-slate-800">{user.position ?? '—'}</dd></div>
					<div><dt class="text-slate-500">Birthday</dt><dd class="mt-0.5 text-slate-800">{formatDate(user.birthday)}</dd></div>
					<div><dt class="text-slate-500">Joined NERP</dt><dd class="mt-0.5 text-slate-800">{formatDate(user.created_at)}</dd></div>
					{#if canManage && user.invite_pending}
						<div><dt class="text-slate-500">Invited</dt><dd class="mt-0.5 text-slate-800">{formatDateTime(user.invited_at)}</dd></div>
					{:else if canManage}
						<div><dt class="text-slate-500">Last sign-in</dt><dd class="mt-0.5 text-slate-800">{formatDateTime(user.last_login_at)}</dd></div>
					{/if}
				</dl>
			</section>

			{#key user.id}<TeamsCard userId={user.id} />{/key}
			{#key user.id}<SkillsCard userId={user.id} canEdit={isMe || canManage} />{/key}

			{#if canManage && !isMe && !user.invite_pending}
				{#key user.id}<SessionsCard userId={user.id} />{/key}
			{/if}

			{#if user.about}
				<section class="card">
					<h2 class="border-b border-slate-100 px-5 py-3.5 font-semibold text-slate-900">About</h2>
					<p class="p-5 text-sm whitespace-pre-line text-slate-700">{user.about}</p>
				</section>
			{/if}
			<ProfileActivity userId={user.id} />
		</div>
	</div>

	<Modal bind:open={editOpen} title="Edit {user.full_name}" size="lg">
		<form id="edit-user" class="grid gap-4 sm:grid-cols-2" onsubmit={saveEdit}>
			<div><label class="label" for="e-fn">First name</label><input id="e-fn" class="input" required bind:value={edit.first_name} /></div>
			<div><label class="label" for="e-ln">Last name</label><input id="e-ln" class="input" required bind:value={edit.last_name} /></div>
			<div><label class="label" for="e-mn">Middle name</label><input id="e-mn" class="input" bind:value={edit.middle_name} /></div>
			<div><label class="label" for="e-em">Email</label><input id="e-em" type="email" class="input" required bind:value={edit.email} /></div>
			<div><label class="label" for="e-pos">Position</label><input id="e-pos" class="input" bind:value={edit.position} /></div>
			<div><label class="label" for="e-bd">Birthday</label><input id="e-bd" type="date" class="input" bind:value={edit.birthday} /></div>
			<div><label class="label" for="e-ph">Work phone</label><input id="e-ph" class="input" bind:value={edit.phone} /></div>
			<div><label class="label" for="e-mb">Mobile</label><input id="e-mb" class="input" bind:value={edit.mobile} /></div>
			<div class="sm:col-span-2">
				<label class="label" for="e-dp">Departments</label>
				<select id="e-dp" class="input" multiple size={Math.min(6, Math.max(3, departments.length))} bind:value={edit.department_ids}>
					{#each departments as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
				</select>
			</div>
		</form>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (editOpen = false)}>Cancel</button>
			<button class="btn btn-primary" form="edit-user" disabled={saving}>Save</button>
		{/snippet}
	</Modal>

	<Modal bind:open={rolesOpen} title="Roles for {user.full_name}">
		<div class="space-y-2">
			{#each roles as r (r.id)}
				<label class="flex items-start gap-3 rounded-lg border border-slate-200 p-3 {r.code === 'employee' ? 'opacity-60' : 'hover:bg-slate-50'}">
					<input
						type="checkbox"
						class="mt-0.5 rounded border-slate-300 text-brand-600 focus:ring-brand-500"
						value={r.id}
						disabled={r.code === 'employee'}
						checked={r.code === 'employee' || selectedRoles.includes(r.id)}
						onchange={(e) =>
							(selectedRoles = e.currentTarget.checked ? [...selectedRoles, r.id] : selectedRoles.filter((x) => x !== r.id))}
					/>
					<span>
						<span class="block text-sm font-medium text-slate-800">{r.name}</span>
						<span class="block text-xs text-slate-500">{r.description ?? `${r.permissions.length} permissions`}</span>
					</span>
				</label>
			{/each}
		</div>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (rolesOpen = false)}>Cancel</button>
			<button class="btn btn-primary" onclick={saveRoles} disabled={saving}>Save roles</button>
		{/snippet}
	</Modal>

	<Modal bind:open={pwOpen} title="Reset password" size="sm">
		<form id="reset-pw" onsubmit={resetPassword} class="space-y-3">
			<p class="text-sm text-slate-600">Set a new password for {user.full_name}. They will be signed out of every device.</p>
			<input class="input font-mono" type="text" minlength="8" required placeholder="New password" bind:value={newPassword} />
		</form>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (pwOpen = false)}>Cancel</button>
			<button class="btn btn-danger" form="reset-pw" disabled={saving}>Reset password</button>
		{/snippet}
	</Modal>
{:else}
	<div class="card p-10 text-center text-slate-400">Loading…</div>
{/if}
