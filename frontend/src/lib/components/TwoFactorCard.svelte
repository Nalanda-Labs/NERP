<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from './Icon.svelte';
	import Modal from './Modal.svelte';

	type Status = { enabled: boolean; enabled_at: string | null; recovery_codes_remaining: number };
	let status = $state<Status | null>(null);

	// setup flow: password → scan + verify → show codes
	let step = $state<'closed' | 'password' | 'scan' | 'codes'>('closed');
	let password = $state('');
	let code = $state('');
	let setup = $state<{ secret: string; qr_png_base64: string } | null>(null);
	let codes = $state<string[]>([]);
	let busy = $state(false);
	let action = $state<'enable' | 'disable' | 'regenerate'>('enable');

	async function load() {
		status = await api.get<Status>('/auth/2fa');
	}
	onMount(load);

	function begin(a: typeof action) {
		action = a;
		password = '';
		code = '';
		step = 'password';
	}

	async function confirmPassword(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			if (action === 'enable') {
				setup = await api.post('/auth/2fa/setup', { password });
				step = 'scan';
			} else if (action === 'disable') {
				await api.post('/auth/2fa/disable', { password });
				toasts.success('Two-factor authentication turned off');
				step = 'closed';
				await finish();
			} else {
				codes = (await api.post<{ recovery_codes: string[] }>('/auth/2fa/recovery-codes', { password })).recovery_codes;
				step = 'codes';
				await load();
			}
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}

	async function verify(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			codes = (await api.post<{ recovery_codes: string[] }>('/auth/2fa/enable', { code })).recovery_codes;
			step = 'codes';
			await finish();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}

	async function finish() {
		await load();
		await session.reloadMe();
	}

	function downloadCodes() {
		const text = `NERP recovery codes for ${session.user?.email}\nEach code works once.\n\n${codes.join('\n')}\n`;
		const a = document.createElement('a');
		a.href = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
		a.download = 'nerp-recovery-codes.txt';
		a.click();
	}

	const open = $derived(step !== 'closed');
	const titles = {
		enable: 'Turn on two-factor authentication',
		disable: 'Turn off two-factor authentication',
		regenerate: 'New recovery codes'
	};
</script>

<section class="card">
	<h2 class="border-b border-slate-100 px-6 py-3.5 font-semibold text-slate-900">Two-factor authentication</h2>
	<div class="flex flex-col gap-4 p-6 sm:flex-row sm:items-center">
		<span class="flex size-11 shrink-0 items-center justify-center rounded-lg {status?.enabled ? 'bg-emerald-50 text-emerald-600' : 'bg-slate-100 text-slate-500'}">
			<Icon name="smartphone" />
		</span>
		<div class="flex-1 text-sm">
			{#if status?.enabled}
				<p class="font-medium text-slate-900">On since {formatDate(status.enabled_at)}</p>
				<p class="text-slate-500">
					Sign-in asks for a code from your authenticator app. {status.recovery_codes_remaining} recovery codes left.
				</p>
			{:else}
				<p class="font-medium text-slate-900">Off</p>
				<p class="text-slate-500">Add a second step at sign-in with an authenticator app such as Google Authenticator, 1Password or Authy.</p>
			{/if}
		</div>
		<div class="flex gap-2">
			{#if status?.enabled}
				<button class="btn btn-secondary" onclick={() => begin('regenerate')}>New recovery codes</button>
				<button class="btn btn-ghost text-red-600 hover:bg-red-50" onclick={() => begin('disable')}>Turn off</button>
			{:else if status}
				<button class="btn btn-primary" onclick={() => begin('enable')}>Turn on</button>
			{/if}
		</div>
	</div>
</section>

<Modal open={open} title={titles[action]} onclose={() => (step = 'closed')}>
	{#if step === 'password'}
		<form id="tfa-pw" class="space-y-3" onsubmit={confirmPassword}>
			<p class="text-sm text-slate-600">Confirm your password to continue.</p>
			<!-- svelte-ignore a11y_autofocus -->
			<input class="input" type="password" autocomplete="current-password" required autofocus bind:value={password} placeholder="Password" />
		</form>
	{:else if step === 'scan' && setup}
		<form id="tfa-verify" class="space-y-4" onsubmit={verify}>
			<ol class="list-decimal space-y-1 pl-5 text-sm text-slate-600">
				<li>Open your authenticator app and add a new account.</li>
				<li>Scan this QR code, or type the key below.</li>
				<li>Enter the 6-digit code the app shows.</li>
			</ol>
			<div class="flex justify-center">
				<img src="data:image/png;base64,{setup.qr_png_base64}" alt="QR code for your authenticator app" class="size-48 rounded-lg border border-slate-200" />
			</div>
			<p class="rounded-xl bg-surface-container p-3 text-center font-mono text-xs break-all text-slate-700 select-all">{setup.secret}</p>
			<input class="input text-center font-mono text-lg tracking-[0.5em]" inputmode="numeric" autocomplete="one-time-code" maxlength="7" required placeholder="000000" bind:value={code} />
		</form>
	{:else if step === 'codes'}
		<div class="space-y-4">
			<p class="rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-800">
				Save these recovery codes somewhere safe. Each one lets you sign in once if you lose your phone. They won't be shown again.
			</p>
			<ul class="grid grid-cols-2 gap-2 rounded-lg bg-slate-50 p-4 font-mono text-sm text-slate-800">
				{#each codes as c (c)}<li>{c}</li>{/each}
			</ul>
			<div class="flex gap-2">
				<button class="btn btn-secondary flex-1" onclick={() => navigator.clipboard.writeText(codes.join('\n')).then(() => toasts.success('Copied'))}>Copy</button>
				<button class="btn btn-secondary flex-1" onclick={downloadCodes}><Icon name="download" class="size-4" /> Download</button>
			</div>
		</div>
	{/if}
	{#snippet footer()}
		{#if step === 'password'}
			<button class="btn btn-secondary" onclick={() => (step = 'closed')}>Cancel</button>
			<button class="btn {action === 'disable' ? 'btn-danger' : 'btn-primary'}" form="tfa-pw" disabled={busy}>Continue</button>
		{:else if step === 'scan'}
			<button class="btn btn-secondary" onclick={() => (step = 'closed')}>Cancel</button>
			<button class="btn btn-primary" form="tfa-verify" disabled={busy}>Verify and turn on</button>
		{:else}
			<button class="btn btn-primary" onclick={() => (step = 'closed')}>I've saved my codes</button>
		{/if}
	{/snippet}
</Modal>
