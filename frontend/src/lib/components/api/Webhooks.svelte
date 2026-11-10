<script lang="ts">
	// Outgoing webhooks: which events go to which URL, the signing secret, a test button and
	// each webhook's delivery log (with redelivery).
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';

	type Hook = {
		id: string; name: string; url: string; secret: string; events: string[]; is_active: boolean;
		delivered_24h: number; failed_24h: number; pending: number; last_delivery_at: string | null;
	};
	type Delivery = {
		id: string; event: string; status: 'pending' | 'done' | 'failed'; attempts: number; response_status: number | null; response_body: string | null;
		error: string | null; next_attempt_at: string | null; delivered_at: string | null; created_at: string; payload: unknown;
	};
	let hooks = $state<Hook[]>([]);
	let events = $state<{ code: string; label: string }[]>([]);
	let loaded = $state(false);
	async function load() {
		try {
			[hooks, events] = await Promise.all([api.get<{ items: Hook[] }>('/webhooks').then((r) => r.items), events.length ? events : api.get<{ items: typeof events }>('/webhooks/events').then((r) => r.items)]);
			loaded = true;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);

	// ----- editor -----
	let open = $state(false);
	let draft = $state<{ id: string | null; name: string; url: string; events: string[]; is_active: boolean; secret: string }>({ id: null, name: '', url: 'https://', events: [], is_active: true, secret: '' });
	let saving = $state(false);
	function edit(h: Hook | null) {
		draft = h ? { id: h.id, name: h.name, url: h.url, events: [...h.events], is_active: h.is_active, secret: h.secret } : { id: null, name: '', url: 'https://', events: ['crm.item.created'], is_active: true, secret: '' };
		open = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			const body = { name: draft.name, url: draft.url, events: draft.events, is_active: draft.is_active };
			if (draft.id) await api.patch(`/webhooks/${draft.id}`, body);
			else await api.post('/webhooks', body);
			open = false;
			toasts.success('Webhook saved');
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
	async function rotate() {
		if (!draft.id || !confirm('Make a new signing secret? The receiver must switch to it.')) return;
		const h = await api.patch<Hook>(`/webhooks/${draft.id}`, { rotate_secret: true });
		draft.secret = h.secret;
		await load();
	}
	async function remove() {
		if (!draft.id || !confirm(`Delete “${draft.name}” and its delivery log?`)) return;
		await api.del(`/webhooks/${draft.id}`);
		open = false;
		await load();
	}
	async function toggle(h: Hook, on: boolean) {
		try {
			await api.patch(`/webhooks/${h.id}`, { is_active: on });
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function test(h: Hook) {
		try {
			await api.post(`/webhooks/${h.id}/test`, {});
			toasts.success('Test event sent');
			setTimeout(() => (log?.hook.id === h.id ? loadLog() : load()), 1500);
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- delivery log -----
	let log = $state<{ hook: Hook; items: Delivery[]; status: string } | null>(null);
	let expanded = $state<string | null>(null);
	async function openLog(h: Hook) {
		log = { hook: h, items: [], status: '' };
		await loadLog();
	}
	async function loadLog() {
		if (!log) return;
		const r = await api.get<{ items: Delivery[] }>(`/webhooks/${log.hook.id}/deliveries?limit=100${log.status ? `&status=${log.status}` : ''}`);
		log.items = r.items;
	}
	async function redeliver(d: Delivery) {
		try {
			await api.post(`/webhook-deliveries/${d.id}/redeliver`, {});
			toasts.success('Sent again');
			setTimeout(loadLog, 1500);
		} catch (e) {
			toasts.error(e);
		}
	}
	const BADGE: Record<string, string> = { done: 'bg-emerald-50 text-emerald-800', failed: 'bg-red-50 text-red-700', pending: 'bg-amber-50 text-amber-800' };
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<p class="flex-1 text-sm text-slate-600">
			Each event is POSTed as JSON with <code class="rounded bg-slate-100 px-1">X-NERP-Event</code> and <code class="rounded bg-slate-100 px-1">X-NERP-Signature: sha256=…</code> (an HMAC of the body with the webhook's secret). Failures with 5xx or no answer are retried for about 8 hours.
		</p>
		<button class="btn btn-primary" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> New webhook</button>
	</div>

	{#if hooks.length}
		<div class="card divide-y divide-slate-100">
			{#each hooks as h (h.id)}
				<div class="flex flex-wrap items-center gap-3 px-5 py-3 text-sm {h.is_active ? '' : 'opacity-60'}" data-hook={h.id}>
					<button class="min-w-0 flex-1 text-left" onclick={() => edit(h)}>
						<p class="font-medium text-slate-900">{h.name}</p>
						<p class="truncate font-mono text-xs text-slate-500">{h.url}</p>
						<p class="mt-0.5 text-xs text-slate-500">{h.events.join(', ')}</p>
					</button>
					<div class="text-right text-xs text-slate-500">
						<p><span class="text-emerald-700">{h.delivered_24h} delivered</span>{#if h.failed_24h} · <span class="font-medium text-red-700">{h.failed_24h} failed</span>{/if}{#if h.pending} · {h.pending} waiting{/if} <span class="text-slate-400">(24 h)</span></p>
						<p>{h.last_delivery_at ? `last ${formatRelative(h.last_delivery_at)}` : 'nothing sent yet'}</p>
					</div>
					<label class="flex items-center gap-1 text-xs text-slate-500"><input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={h.is_active} onchange={(e) => toggle(h, e.currentTarget.checked)} /> Active</label>
					<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => test(h)}><Icon name="send" class="size-3.5" /> Send test</button>
					<button class="btn btn-secondary px-3 py-1 text-xs" onclick={() => openLog(h)}>Deliveries</button>
				</div>
			{/each}
		</div>
	{:else if loaded}
		<div class="card p-8 text-center text-sm text-slate-500">No webhooks yet.</div>
	{/if}
</div>

<Modal bind:open title={draft.id ? 'Edit webhook' : 'New webhook'} size="lg">
	<form id="hook-form" class="space-y-4 text-sm" onsubmit={save}>
		<div class="grid gap-3 sm:grid-cols-2">
			<div><label class="label" for="w-name">Name</label><input id="w-name" class="input" required maxlength="100" bind:value={draft.name} placeholder="e.g. Data warehouse" /></div>
			<div><label class="label" for="w-url">URL</label><input id="w-url" class="input font-mono text-xs" required bind:value={draft.url} /></div>
		</div>
		<fieldset>
			<legend class="label">Events</legend>
			<div class="grid gap-1.5 sm:grid-cols-2">
				{#each events as ev (ev.code)}
					<label class="flex items-start gap-2" title={ev.label}>
						<input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" checked={draft.events.includes(ev.code)} onchange={(e) => (draft.events = e.currentTarget.checked ? [...draft.events, ev.code] : draft.events.filter((x) => x !== ev.code))} />
						<span><code class="text-xs">{ev.code}</code><span class="block text-xs text-slate-500">{ev.label}</span></span>
					</label>
				{/each}
			</div>
		</fieldset>
		{#if draft.id}
			<div>
				<span class="label">Signing secret</span>
				<div class="flex gap-2">
					<code class="min-w-0 flex-1 rounded-lg bg-slate-100 px-3 py-2 font-mono text-xs break-all" data-secret>{draft.secret}</code>
					<button type="button" class="btn btn-secondary" onclick={() => navigator.clipboard?.writeText(draft.secret).then(() => toasts.success('Copied'))}>Copy</button>
					<button type="button" class="btn btn-ghost" onclick={rotate}>New secret</button>
				</div>
				<p class="mt-1 text-xs text-slate-500">Check it on your side: HMAC-SHA256 of the raw request body with this secret, hex, prefixed with <code>sha256=</code>.</p>
			</div>
		{:else}
			<p class="text-xs text-slate-500">A signing secret is made when you save.</p>
		{/if}
		<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.is_active} /> Active</label>
	</form>
	{#snippet footer()}
		{#if draft.id}<button class="btn btn-ghost mr-auto text-red-600" onclick={remove}><Icon name="trash" class="size-4" /> Delete</button>{/if}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" type="submit" form="hook-form" disabled={saving || !draft.events.length}>{saving ? 'Saving…' : 'Save'}</button>
	{/snippet}
</Modal>

{#if log}
	{@const l = log}
	<Modal open={true} title="Deliveries: {l.hook.name}" size="lg" onclose={() => ((log = null), load())}>
		<div class="space-y-3 text-sm">
			<div class="flex items-center gap-2">
				<select class="input w-40" aria-label="Status" value={l.status} onchange={(e) => { log!.status = e.currentTarget.value; loadLog(); }}><option value="">All</option><option value="done">Delivered</option><option value="pending">Waiting</option><option value="failed">Failed</option></select>
				<button class="btn btn-ghost px-2 py-1 text-xs" onclick={loadLog}><Icon name="refresh" class="size-3.5" /> Refresh</button>
			</div>
			<ul class="max-h-[60vh] divide-y divide-slate-100 overflow-y-auto rounded-xl border border-slate-200">
				{#each l.items as d (d.id)}
					<li class="px-3 py-2" data-delivery={d.id}>
						<button class="flex w-full items-center gap-3 text-left" onclick={() => (expanded = expanded === d.id ? null : d.id)}>
							<span class="rounded-full px-2 py-0.5 text-xs font-medium {BADGE[d.status]}">{d.status === 'done' ? 'Delivered' : d.status === 'pending' ? 'Waiting' : 'Failed'}</span>
							<code class="text-xs text-slate-700">{d.event}</code>
							<span class="flex-1 truncate text-xs text-slate-500">{d.response_status ? `HTTP ${d.response_status}` : ''}{d.error ? ` · ${d.error}` : ''}</span>
							<span class="text-xs whitespace-nowrap text-slate-500">{formatDateTime(d.created_at)}</span>
						</button>
						{#if expanded === d.id}
							<div class="mt-2 space-y-2 text-xs">
								<p class="text-slate-600">{d.attempts} attempt{d.attempts === 1 ? '' : 's'}{d.next_attempt_at ? ` · next ${formatDateTime(d.next_attempt_at)}` : ''}{d.delivered_at ? ` · delivered ${formatDateTime(d.delivered_at)}` : ''}</p>
								<pre class="max-h-60 overflow-auto rounded-lg bg-slate-900 p-2 text-slate-100">{JSON.stringify(d.payload, null, 2)}</pre>
								{#if d.response_body}<p class="text-slate-500">Response: <span class="font-mono">{d.response_body.slice(0, 300)}</span></p>{/if}
								<button class="btn btn-secondary px-2 py-1 text-xs" onclick={() => redeliver(d)}>Send again</button>
							</div>
						{/if}
					</li>
				{:else}
					<li class="px-3 py-6 text-center text-slate-500">Nothing here.</li>
				{/each}
			</ul>
		</div>
		{#snippet footer()}<button class="btn btn-primary" onclick={() => ((log = null), load())}>Close</button>{/snippet}
	</Modal>
{/if}
