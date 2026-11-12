<script lang="ts">
	// System → Health: whether the installation is in good shape, a self-test of its moving
	// parts, and who is alerted when a check turns red.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Level = 'off' | 'ok' | 'warn' | 'fail';
	type Check = { key: string; title: string; level: Level; text: string; link?: string };
	type Run = { checked_at: string; overall: Level; checks: Check[] };
	type Info = { last: Run; self_test: Run | null; settings: { alerts: boolean; extra_emails: string[] }; every_minutes: number };
	let info = $state<Info | null>(null);
	let running = $state(false);
	let extra = $state('');
	let alerts = $state(true);

	function take(x: Info) {
		info = x;
		alerts = x.settings.alerts;
		extra = x.settings.extra_emails.join(', ');
	}
	onMount(async () => {
		try {
			take(await api.get<Info>('/admin/health'));
		} catch (e) {
			toasts.error(e);
		}
	});
	async function runNow() {
		running = true;
		try {
			take(await api.post<Info>('/admin/health/run', {}));
		} catch (e) {
			toasts.error(e);
		} finally {
			running = false;
		}
	}
	async function save() {
		try {
			const s = await api.put<Info['settings']>('/admin/health/settings', { alerts, extra_emails: extra.split(/[,\s]+/).filter(Boolean) });
			extra = s.extra_emails.join(', ');
			toasts.success('Saved');
		} catch (e) {
			toasts.error(e);
		}
	}
	const DOT: Record<Level, string> = { ok: 'bg-emerald-500', warn: 'bg-amber-500', fail: 'bg-red-500', off: 'bg-slate-300' };
	const WORD: Record<Level, string> = { ok: 'All good', warn: 'Needs a look', fail: 'Needs attention', off: 'Not checked' };
	const BANNER: Record<Level, string> = { ok: 'bg-emerald-50 text-emerald-900', warn: 'bg-amber-50 text-amber-900', fail: 'bg-red-50 text-red-900', off: 'bg-slate-50 text-slate-700' };
</script>

<svelte:head><title>Health · NERP</title></svelte:head>

<div class="max-w-4xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Health</h1>
			<p class="text-sm text-slate-500"><a class="text-brand-600 hover:underline" href="/system">System</a> · checked every {info?.every_minutes ?? 15} minutes{info ? `, last ${formatDateTime(info.last.checked_at)}` : ''}</p>
		</div>
		<button class="btn btn-primary ml-auto" disabled={running} onclick={runNow} data-run-health>{running ? 'Testing…' : 'Check and self-test now'}</button>
	</div>

	{#if info}
		<div class="rounded-xl px-5 py-3 text-sm font-medium {BANNER[info.last.overall]}" data-overall={info.last.overall}>{WORD[info.last.overall]}</div>

		{#snippet list(run: Run, label: string)}
			<section class="card divide-y divide-slate-100" aria-label={label}>
				{#each run.checks as c (c.key)}
					<div class="flex items-start gap-3 px-5 py-3 text-sm" data-check={c.key} data-level={c.level}>
						<span class="mt-1.5 h-2.5 w-2.5 shrink-0 rounded-full {DOT[c.level]}" title={WORD[c.level]}></span>
						<div class="min-w-0 flex-1">
							<p class="font-medium text-slate-900">{c.title}</p>
							<p class="text-slate-600">{c.text}</p>
						</div>
						{#if c.link}<a class="shrink-0 text-xs text-brand-700 hover:underline" href={c.link}>Open</a>{/if}
					</div>
				{/each}
			</section>
		{/snippet}

		{@render list(info.last, 'Checks')}

		<h2 class="pt-2 text-sm font-semibold text-slate-900">Self-test</h2>
		{#if info.self_test}
			<p class="-mt-3 text-xs text-slate-500">{formatDateTime(info.self_test.checked_at)}: writes and reads back a value in the database and Redis and a file in storage, and connects to the mail server.</p>
			{@render list(info.self_test, 'Self-test')}
		{:else}
			<p class="-mt-3 text-sm text-slate-500">Not run yet. "Check and self-test now" writes and reads back a value in the database and Redis and a file in storage, and connects to the mail server.</p>
		{/if}

		<section class="card space-y-3 p-5 text-sm" aria-label="Alerts">
			<h2 class="font-semibold text-slate-900">Alerts</h2>
			<label class="flex items-center gap-2"><input type="checkbox" bind:checked={alerts} data-alerts /> Email those who manage the system when a check turns amber or red, and when all is well again</label>
			<label class="block">
				<span class="text-slate-600">Also send to (an outside mailbox helps when NERP's own mail is the problem)</span>
				<input class="input mt-1" placeholder="ops@example.com" bind:value={extra} data-extra-emails />
			</label>
			<button class="btn btn-secondary" onclick={save}>Save</button>
		</section>
	{:else}
		<p class="text-sm text-slate-400">Checking…</p>
	{/if}
</div>
