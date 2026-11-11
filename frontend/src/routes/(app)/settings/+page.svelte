<script lang="ts">
	import { onMount } from 'svelte';
	import { api, fileUrl } from '#lib/api.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { Settings } from '#lib/types.ts';

	let settings = $state<Settings | null>(null);
	let saving = $state(false);
	const timezones: string[] = Intl.supportedValuesOf?.('timeZone') ?? ['UTC'];

	let ipText = $state('');
	let retention = $state('');
	onMount(async () => {
		settings = await api.get<Settings>('/settings');
		ipText = (settings.ip_allow ?? []).join('\n');
		retention = settings.chat_retention_days == null ? '' : String(settings.chat_retention_days);
	});

	let uploadingLogo = $state(false);
	async function uploadLogo(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		if (!file || !settings) return;
		uploadingLogo = true;
		try {
			const fd = new FormData();
			fd.append('file', file);
			// Public, so it can be shown in <img> and printed on documents.
			fd.append('public', 'true');
			const f = await api.post<{ id: string }>('/files', fd);
			settings.company_logo_file_id = f.id;
		} catch (err) {
			toasts.error(err);
		} finally {
			uploadingLogo = false;
			input.value = '';
		}
	}

	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			settings!.ip_allow = ipText
				.split(/[\n,]/)
				.map((x) => x.trim())
				.filter(Boolean);
			settings!.chat_retention_days = retention ? Number(retention) : null;
			settings = await api.put<Settings>('/settings', settings!);
			session.companyName = settings.company_name ?? session.companyName;
			toasts.success('Settings saved');
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
</script>

<svelte:head><title>Settings · NERP</title></svelte:head>

<div class="max-w-2xl space-y-5">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">Settings</h1>
		<p class="text-sm text-slate-500">Company-wide preferences.</p>
	</div>

	{#if settings}
		<form class="card" onsubmit={save}>
			<div class="space-y-5 p-6">
				<div>
					<label class="label" for="s-company">Company name</label>
					<input
						id="s-company"
						class="input"
						required
						maxlength="200"
						bind:value={settings.company_name}
					/>
				</div>
				<div class="grid gap-4 sm:grid-cols-2">
					<div>
						<label class="label" for="s-tz">Default time zone</label>
						<select id="s-tz" class="input" bind:value={settings.default_timezone}>
							{#if !timezones.includes(settings.default_timezone!)}<option
									>{settings.default_timezone}</option
								>{/if}
							{#each timezones as tz (tz)}<option>{tz}</option>{/each}
						</select>
					</div>
					<div>
						<label class="label" for="s-locale">Default language</label>
						<select id="s-locale" class="input" bind:value={settings.default_locale}>
							<option value="en">English</option>
						</select>
					</div>
					<div>
						<label class="label" for="s-date">Date format</label>
						<select id="s-date" class="input" bind:value={settings.date_format}>
							{#each ['YYYY-MM-DD', 'DD.MM.YYYY', 'DD/MM/YYYY', 'MM/DD/YYYY'] as f (f)}<option
									>{f}</option
								>{/each}
						</select>
					</div>
					<div>
						<label class="label" for="s-week">First day of the week</label>
						<select id="s-week" class="input" bind:value={settings.week_start}>
							<option value="monday">Monday</option>
							<option value="sunday">Sunday</option>
						</select>
					</div>
				</div>
			</div>
			<div class="space-y-2 border-t border-slate-100 p-6">
				<h2 class="font-medium text-slate-900">Security</h2>
				<label class="flex items-start gap-3 text-sm">
					<input
						type="checkbox"
						class="mt-0.5 rounded border-slate-300 text-brand-600"
						bind:checked={settings.require_two_factor}
						data-require-2fa
					/>
					<span>
						<span class="block font-medium text-slate-800"
							>Require two-step sign-in for everyone</span
						>
						<span class="text-slate-500"
							>After their password (or company sign-on), people also enter a code from an
							authenticator app on their phone. Anyone who hasn't set it up is asked for their
							mobile number and to set it up at their next sign-in.</span
						>
					</span>
				</label>
				<div class="pt-2">
					<label class="label" for="s-ip">Use NERP only from these networks</label>
					<textarea
						id="s-ip"
						class="input font-mono text-xs"
						rows="3"
						placeholder="Anywhere (one address or range per line, such as 192.168.1.0/24)"
						bind:value={ipText}></textarea>
					<p class="mt-1 text-xs text-slate-500">
						Applies to signing in, the app, API keys and calendar apps; not to public forms and
						links. Saving a list that leaves out your own address is refused.
					</p>
					<label class="mt-2 flex items-center gap-2 text-sm"
						><input
							type="checkbox"
							class="rounded border-slate-300 text-brand-600"
							bind:checked={settings.ip_allow_admins_exempt}
						/> Administrators may sign in from anywhere</label
					>
				</div>
				<label class="flex items-start gap-3 pt-2 text-sm">
					<input
						type="checkbox"
						class="mt-0.5 rounded border-slate-300 text-brand-600"
						bind:checked={settings.public_links}
						data-public-links
					/>
					<span>
						<span class="block font-medium text-slate-800">Links for anyone</span>
						<span class="text-slate-500"
							>People may make links that open files, boards and BI dashboards without signing in.
							Switched off, no new links can be made and existing ones stop working (they work again
							if switched back on).</span
						>
					</span>
				</label>
				<div class="pt-2">
					<label class="label" for="s-chat">Delete chat messages older than (days)</label>
					<input
						id="s-chat"
						class="input w-40"
						type="number"
						min="1"
						max="3650"
						placeholder="Keep them"
						bind:value={retention}
					/>
					<p class="mt-1 text-xs text-slate-500">
						For every chat; chats can also set a shorter time of their own.
					</p>
				</div>
			</div>
			<div class="space-y-4 border-t border-slate-100 p-6">
				<div>
					<h2 class="font-medium text-slate-900">Company details on documents</h2>
					<p class="text-sm text-slate-500">
						Printed on quotes and invoices created from CRM templates.
					</p>
				</div>
				<div class="flex items-center gap-4">
					{#if settings.company_logo_file_id}
						<img
							src={fileUrl(settings.company_logo_file_id)}
							alt="Company logo"
							class="h-12 max-w-48 rounded object-contain"
						/>
					{:else}
						<div
							class="flex h-12 w-24 items-center justify-center rounded border border-dashed border-slate-300 text-xs text-slate-400"
						>
							No logo
						</div>
					{/if}
					<label class="btn btn-secondary cursor-pointer text-sm"
						>{uploadingLogo ? 'Uploading…' : 'Upload logo'}<input
							type="file"
							class="sr-only"
							accept="image/png,image/jpeg,image/svg+xml"
							onchange={uploadLogo}
						/></label
					>
					{#if settings.company_logo_file_id}<button
							type="button"
							class="btn btn-ghost text-sm"
							onclick={() => (settings!.company_logo_file_id = '')}>Remove</button
						>{/if}
				</div>
				<div class="grid gap-4 sm:grid-cols-2">
					<div>
						<label class="label" for="s-legal">Legal name</label>
						<input
							id="s-legal"
							class="input"
							maxlength="2000"
							bind:value={settings.company_legal_name}
							placeholder={settings.company_name}
						/>
					</div>
					<div>
						<label class="label" for="s-taxid">Tax / VAT ID</label>
						<input
							id="s-taxid"
							class="input"
							maxlength="2000"
							bind:value={settings.company_tax_id}
						/>
					</div>
					<div>
						<label class="label" for="s-regno">Registration number</label>
						<input
							id="s-regno"
							class="input"
							maxlength="2000"
							bind:value={settings.company_registration_no}
						/>
					</div>
					<div>
						<label class="label" for="s-phone">Phone</label>
						<input
							id="s-phone"
							class="input"
							maxlength="2000"
							bind:value={settings.company_phone}
						/>
					</div>
					<div>
						<label class="label" for="s-email">Email</label>
						<input
							id="s-email"
							class="input"
							type="email"
							maxlength="2000"
							bind:value={settings.company_email}
						/>
					</div>
					<div>
						<label class="label" for="s-web">Website</label>
						<input
							id="s-web"
							class="input"
							maxlength="2000"
							bind:value={settings.company_website}
						/>
					</div>
					<div class="sm:col-span-2">
						<label class="label" for="s-address">Address</label>
						<textarea
							id="s-address"
							class="input"
							rows="2"
							maxlength="2000"
							bind:value={settings.company_address}></textarea>
					</div>
					<div class="sm:col-span-2">
						<label class="label" for="s-bank">Payment details</label>
						<textarea
							id="s-bank"
							class="input"
							rows="2"
							maxlength="2000"
							bind:value={settings.company_bank_details}
							placeholder="Bank, IBAN / account number, SWIFT / IFSC…"></textarea>
					</div>
				</div>
			</div>
			<div class="flex justify-end border-t border-slate-100 px-6 py-3">
				<button class="btn btn-primary" disabled={saving}
					>{saving ? 'Saving…' : 'Save settings'}</button
				>
			</div>
		</form>
	{/if}
</div>
