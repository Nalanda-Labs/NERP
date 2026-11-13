<script lang="ts">
	// System → Text messages: the Twilio sender, the calling code for local numbers, the
	// addresses to give Twilio, a test message, and every message sent or received.
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Setup = { account: string | null; from: string; from_env: string; country_code: string; enabled: boolean; status_url: string; inbound_url: string };
	type Message = { id: string; direction: 'in' | 'out'; to_number: string; from_number: string | null; body: string; status: string; error: string | null; purpose: string; item_id: string | null; user_name: string | null; created_at: string };

	let setup = $state<Setup | null>(null);
	let form = $state({ from: '', country_code: '' });
	let testTo = $state('');
	let search = $state('');
	let messages = $state<Message[]>([]);
	let saving = $state(false);

	async function load() {
		try {
			setup = await api.get<Setup>('/sms/setup');
			form = { from: setup.from, country_code: setup.country_code };
			await loadMessages();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function loadMessages() {
		messages = (await api.get<{ messages: Message[] }>('/sms/messages' + query({ q: search || null }))).messages;
	}
	onMount(load);

	async function save(e: Event) {
		e.preventDefault();
		saving = true;
		try {
			setup = await api.put<Setup>('/sms/setup', form);
			toasts.success('Saved');
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
	async function test(e: Event) {
		e.preventDefault();
		try {
			await api.post('/sms/test', { to: testTo });
			toasts.success('Test message queued');
			setTimeout(loadMessages, 1500);
		} catch (err) {
			toasts.error(err);
		}
	}
	const STATUS: Record<string, string> = {
		queued: 'bg-slate-100 text-slate-700',
		sending: 'bg-sky-50 text-sky-700',
		sent: 'bg-sky-50 text-sky-700',
		delivered: 'bg-emerald-50 text-emerald-700',
		received: 'bg-violet-50 text-violet-700',
		undelivered: 'bg-amber-50 text-amber-800',
		failed: 'bg-red-50 text-red-700'
	};
</script>

<svelte:head><title>Text messages · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<div>
		<h1 class="text-2xl font-semibold text-slate-900">Text messages</h1>
		<p class="text-sm text-slate-500"><a class="text-brand-600 hover:underline" href="/system">System</a> · SMS through Twilio: from CRM records, robots, booking reminders, signature requests, campaigns and invitations.</p>
	</div>

	{#if setup}
		<section class="card space-y-4 p-5 text-sm" aria-label="Set-up">
			{#if !setup.account}
				<p class="text-slate-600" data-sms-missing>Set <code>TWILIO_ACCOUNT_SID</code> and <code>TWILIO_AUTH_TOKEN</code> in the server's settings (<code>backend/.env</code>) and restart NERP. They are never stored in the database.</p>
			{:else}
				<p class="text-slate-600">Twilio account <code>{setup.account}</code> · <span class={setup.enabled ? 'text-emerald-700' : 'text-amber-700'} data-sms-enabled={setup.enabled}>{setup.enabled ? 'ready to send' : 'choose a sender to start sending'}</span></p>
				<form class="grid gap-4 sm:grid-cols-[1fr_12rem_auto] sm:items-end" onsubmit={save}>
					<label class="block">
						<span class="label">Sender</span>
						<input class="input" id="sms-from" placeholder={setup.from_env || '+1 555 010 7788 or MG…'} bind:value={form.from} />
						<span class="text-xs text-slate-500">A Twilio number in international format, or a Messaging Service SID{setup.from_env ? ` (empty: ${setup.from_env} from the server settings)` : ''}.</span>
					</label>
					<label class="block">
						<span class="label">Calling code for local numbers</span>
						<input class="input" id="sms-cc" placeholder="+91" bind:value={form.country_code} />
						<span class="text-xs text-slate-500">Empty: numbers need their +code.</span>
					</label>
					<button class="btn btn-primary" disabled={saving}>Save</button>
				</form>
				<div class="rounded-lg bg-slate-50 p-3">
					<p class="font-medium text-slate-800">In Twilio, for your number (Phone Numbers → Configure → Messaging):</p>
					<p class="mt-1">A message comes in → Webhook, HTTP POST: <code class="break-all" data-inbound-url>{setup.inbound_url}</code></p>
					<p class="mt-1 text-xs text-slate-500">Delivery reports come back to <code class="break-all">{setup.status_url}</code> by themselves. NERP must be reachable from the internet at that address.</p>
				</div>
				{#if setup.enabled}
					<form class="flex flex-wrap items-end gap-3" onsubmit={test}>
						<label class="block"><span class="label">Send a test to</span><input class="input w-56" id="sms-test-to" type="tel" placeholder="+44 20 7946 0000" bind:value={testTo} /></label>
						<button class="btn" disabled={!testTo.trim()}>Send test</button>
					</form>
				{/if}
			{/if}
		</section>

		<section class="card p-5 text-sm" aria-label="Messages">
			<div class="flex flex-wrap items-center justify-between gap-3">
				<h2 class="font-semibold text-slate-900">Messages</h2>
				<input class="input w-64" type="search" placeholder="Search numbers and text" bind:value={search} oninput={() => loadMessages().catch(toasts.error)} />
			</div>
			<div class="mt-3 overflow-x-auto">
				<table class="w-full text-left" data-sms-log>
					<thead class="text-xs text-slate-500"><tr><th class="py-2 pr-3 font-medium">When</th><th class="pr-3 font-medium">Number</th><th class="pr-3 font-medium">Message</th><th class="pr-3 font-medium">Status</th><th class="font-medium">For</th></tr></thead>
					<tbody class="divide-y divide-slate-100">
						{#each messages as m (m.id)}
							<tr data-sms={m.id}>
								<td class="py-2 pr-3 whitespace-nowrap text-slate-500">{formatDateTime(m.created_at)}</td>
								<td class="pr-3 whitespace-nowrap">{m.direction === 'in' ? '← ' + (m.from_number ?? '') : '→ ' + m.to_number}</td>
								<td class="max-w-md pr-3"><p class="line-clamp-2 whitespace-pre-line">{m.body}</p>{#if m.error}<p class="text-xs text-red-700">{m.error}</p>{/if}</td>
								<td class="pr-3"><span class="rounded px-1.5 py-0.5 text-xs {STATUS[m.status] ?? 'bg-slate-100'}" data-sms-status>{m.status}</span></td>
								<td class="text-slate-500">{m.purpose}{#if m.user_name} · {m.user_name}{/if}</td>
							</tr>
						{:else}
							<tr><td colspan="5" class="py-3 text-slate-500">No messages yet.</td></tr>
						{/each}
					</tbody>
				</table>
			</div>
		</section>
	{/if}
</div>
