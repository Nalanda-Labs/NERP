<script lang="ts">
	// System → Sandbox: refresh the anonymised copy (on the live system), or — in the sandbox —
	// see the emails it kept instead of sending.
	import { onDestroy, onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Status = { is_sandbox: boolean; configured: boolean; url: string | null; running: boolean; last: { started_at: string; finished_at: string; ok: boolean; output: string } | null };
	let st = $state<Status | null>(null);
	let outbox = $state<{ to: string; subject: string; text: string; at: string }[]>([]);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function load() {
		try {
			st = await api.get<Status>('/admin/sandbox');
			if (st.is_sandbox) outbox = (await api.get<{ items: typeof outbox }>('/admin/sandbox/outbox')).items;
			clearTimeout(timer);
			if (st.running) timer = setTimeout(load, 3000);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	onDestroy(() => clearTimeout(timer));
	async function refresh() {
		try {
			await api.post('/admin/sandbox/refresh', {});
			toasts.success('Refreshing the sandbox');
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>Sandbox · NERP</title></svelte:head>

<div class="max-w-4xl space-y-5">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">Sandbox</h1>
		<p class="text-sm text-slate-500"><a class="text-brand-600 hover:underline" href="/system">System</a> · an anonymised copy of the company to try automation, imports and settings on, without touching real clients.</p>
	</div>
	{#if st?.is_sandbox}
		<section class="card p-5 text-sm" aria-label="Outbox">
			<h2 class="font-semibold text-slate-900">Emails the sandbox didn't send</h2>
			<ul class="mt-2 divide-y divide-slate-100" data-outbox>
				{#each outbox as m, i (i)}<li class="py-2"><p><b class="font-medium">{m.subject}</b> → {m.to}</p><p class="text-xs text-slate-500">{formatDateTime(m.at)}</p></li>{:else}<li class="py-2 text-slate-500">None yet.</li>{/each}
			</ul>
		</section>
	{:else if st}
		<section class="card space-y-3 p-5 text-sm" aria-label="Sandbox copy">
			{#if !st.configured}
				<p class="text-slate-600">Set <code>SANDBOX_DATABASE_URL</code> to a database for the copy and run a second NERP on it with <code>SANDBOX=true</code> (see SETUP.md).</p>
			{:else}
				<p class="text-slate-600">Refreshing replaces the sandbox with a fresh copy of today's data: clients' names, contact details, addresses, notes, emails, chats, tickets and form data are made up or removed; connections and keys are switched off; employees sign in as usual. Sandboxes send no email and call nothing outside.</p>
				<div class="flex flex-wrap items-center gap-3">
					<button class="btn btn-primary" disabled={st.running} onclick={refresh} data-sandbox-refresh>{st.running ? 'Refreshing…' : 'Refresh the sandbox'}</button>
					{#if st.url}<a class="text-brand-700 hover:underline" href={st.url} target="_blank" rel="noopener">Open the sandbox</a>{/if}
				</div>
			{/if}
			{#if st.last}
				<div data-sandbox-last={st.last.ok ? 'ok' : 'failed'}>
					<p class={st.last.ok ? 'text-emerald-700' : 'text-red-700'}>{st.last.ok ? 'Refreshed' : 'Failed'} {formatDateTime(st.last.finished_at)}</p>
					<pre class="mt-1 max-h-64 overflow-auto rounded bg-slate-50 p-2 text-xs whitespace-pre-wrap">{st.last.output}</pre>
				</div>
			{/if}
		</section>
	{/if}
</div>
