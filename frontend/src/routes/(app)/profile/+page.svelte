<script lang="ts">
	import { api } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import AppPasswordsCard from '#lib/components/AppPasswordsCard.svelte';
	import LinkedAccountsCard from '#lib/components/LinkedAccountsCard.svelte';
	import SessionsCard from '#lib/components/SessionsCard.svelte';
	import TwoFactorCard from '#lib/components/TwoFactorCard.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { FileMeta, Session, User } from '#lib/types.ts';

	const me = session.user!;
	let form = $state({
		first_name: me.first_name,
		last_name: me.last_name,
		middle_name: me.middle_name ?? '',
		phone: me.phone ?? '',
		mobile: me.mobile ?? '',
		birthday: me.birthday ?? '',
		gender: me.gender ?? '',
		about: me.about ?? '',
		timezone: me.timezone
	});
	let saving = $state(false);
	let avatarInput: HTMLInputElement | undefined = $state();
	const timezones: string[] = Intl.supportedValuesOf?.('timeZone') ?? ['UTC'];

	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			await api.patch<User>('/profile', { ...form, birthday: form.birthday || null, gender: form.gender || null });
			await session.reloadMe();
			toasts.success('Profile saved');
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}

	async function uploadAvatar(files: FileList | null) {
		const file = files?.[0];
		if (!file) return;
		if (!/^image\/(png|jpeg|gif|webp|avif)$/.test(file.type)) {
			toasts.error(new Error('Choose a PNG, JPEG, GIF, WebP or AVIF image'));
			return;
		}
		const fd = new FormData();
		fd.append('public', 'true');
		fd.append('file', file);
		try {
			const meta = await api.post<FileMeta>('/files', fd);
			await api.patch('/profile', { avatar_file_id: meta.id });
			await session.reloadMe();
			toasts.success('Photo updated');
		} catch (err) {
			toasts.error(err);
		} finally {
			if (avatarInput) avatarInput.value = '';
		}
	}

	async function removeAvatar() {
		try {
			await api.patch('/profile', { avatar_file_id: null });
			await session.reloadMe();
		} catch (err) {
			toasts.error(err);
		}
	}

	// ----- password -----
	let pw = $state({ current_password: '', new_password: '', confirm: '' });
	let pwSaving = $state(false);

	async function changePassword(e: SubmitEvent) {
		e.preventDefault();
		if (pw.new_password !== pw.confirm) {
			toasts.error(new Error('New passwords do not match'));
			return;
		}
		pwSaving = true;
		try {
			const s = await api.post<Session>('/auth/change-password', {
				current_password: pw.current_password,
				new_password: pw.new_password
			});
			session.apply(s);
			pw = { current_password: '', new_password: '', confirm: '' };
			toasts.success('Password changed. Other devices have been signed out.');
		} catch (err) {
			toasts.error(err);
		} finally {
			pwSaving = false;
		}
	}
</script>

<svelte:head><title>My profile · NERP</title></svelte:head>

<div class="max-w-3xl space-y-6">
	<h1 class="text-2xl font-semibold text-slate-900">My profile</h1>

	<section class="card flex flex-col items-center gap-5 p-6 sm:flex-row">
		<Avatar name={session.user!.full_name} fileId={session.user!.avatar_file_id} size="xl" />
		<div class="flex-1 text-center sm:text-left">
			<p class="text-lg font-semibold text-slate-900">{session.user!.full_name}</p>
			<p class="text-sm text-slate-500">{session.user!.email}</p>
			<div class="mt-3 flex justify-center gap-2 sm:justify-start">
				<button class="btn btn-secondary" onclick={() => avatarInput?.click()}><Icon name="upload" class="size-4" /> Upload photo</button>
				{#if session.user!.avatar_file_id}
					<button class="btn btn-ghost" onclick={removeAvatar}>Remove</button>
				{/if}
			</div>
			<input bind:this={avatarInput} type="file" accept="image/png,image/jpeg,image/gif,image/webp,image/avif" class="hidden" onchange={(e) => uploadAvatar(e.currentTarget.files)} />
		</div>
	</section>

	<form class="card" onsubmit={save}>
		<h2 class="border-b border-slate-100 px-6 py-3.5 font-semibold text-slate-900">Personal details</h2>
		<div class="grid gap-4 p-6 sm:grid-cols-2">
			<div><label class="label" for="p-fn">First name</label><input id="p-fn" class="input" required bind:value={form.first_name} /></div>
			<div><label class="label" for="p-ln">Last name</label><input id="p-ln" class="input" required bind:value={form.last_name} /></div>
			<div><label class="label" for="p-mn">Middle name</label><input id="p-mn" class="input" bind:value={form.middle_name} /></div>
			<div><label class="label" for="p-bd">Birthday</label><input id="p-bd" type="date" class="input" bind:value={form.birthday} /></div>
			<div><label class="label" for="p-ph">Work phone</label><input id="p-ph" class="input" bind:value={form.phone} /></div>
			<div><label class="label" for="p-mb">Mobile</label><input id="p-mb" class="input" bind:value={form.mobile} /></div>
			<div>
				<label class="label" for="p-g">Gender</label>
				<select id="p-g" class="input" bind:value={form.gender}>
					<option value="">Prefer not to say</option>
					<option value="female">Female</option>
					<option value="male">Male</option>
					<option value="other">Other</option>
				</select>
			</div>
			<div>
				<label class="label" for="p-tz">Time zone</label>
				<select id="p-tz" class="input" bind:value={form.timezone}>
					{#if !timezones.includes(form.timezone)}<option>{form.timezone}</option>{/if}
					{#each timezones as tz (tz)}<option>{tz}</option>{/each}
				</select>
			</div>
			<div class="sm:col-span-2">
				<label class="label" for="p-about">About me</label>
				<textarea id="p-about" class="input" rows="4" maxlength="4000" bind:value={form.about}></textarea>
			</div>
		</div>
		<div class="flex justify-end border-t border-slate-100 px-6 py-3">
			<button class="btn btn-primary" disabled={saving}>{saving ? 'Saving…' : 'Save profile'}</button>
		</div>
	</form>

	<h2 id="security" class="pt-2 text-lg font-semibold text-slate-900">Security</h2>

	<form class="card" onsubmit={changePassword}>
		<h2 class="border-b border-slate-100 px-6 py-3.5 font-semibold text-slate-900">Change password</h2>
		<div class="grid gap-4 p-6 sm:grid-cols-3">
			<div><label class="label" for="c-cur">Current password</label><input id="c-cur" type="password" autocomplete="current-password" class="input" required bind:value={pw.current_password} /></div>
			<div><label class="label" for="c-new">New password</label><input id="c-new" type="password" autocomplete="new-password" minlength="8" class="input" required bind:value={pw.new_password} /></div>
			<div><label class="label" for="c-cf">Confirm</label><input id="c-cf" type="password" autocomplete="new-password" minlength="8" class="input" required bind:value={pw.confirm} /></div>
		</div>
		<div class="flex justify-end border-t border-slate-100 px-6 py-3">
			<button class="btn btn-primary" disabled={pwSaving}>Change password</button>
		</div>
	</form>

	<TwoFactorCard />
	<LinkedAccountsCard />
	<SessionsCard />
	<AppPasswordsCard />
</div>
