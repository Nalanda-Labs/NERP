<script lang="ts">
	// Texting the client from a CRM record: the conversation so far, the number to use, and a
	// message box that counts characters and SMS parts.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Message = { id: string; direction: 'in' | 'out'; to_number: string; from_number: string | null; body: string; status: string; error: string | null; user_name: string | null; created_at: string };
	let { itemId, onsent, oncancel }: { itemId: string; onsent?: () => void; oncancel?: () => void } = $props();

	let enabled = $state(true);
	let phones = $state<string[]>([]);
	let messages = $state<Message[]>([]);
	let to = $state('');
	let body = $state('');
	let sending = $state(false);
	let loaded = $state(false);

	async function load() {
		const r = await api.get<{ enabled: boolean; phones: string[]; messages: Message[] }>(`/crm/items/${itemId}/sms`);
		enabled = r.enabled;
		phones = r.phones;
		messages = r.messages;
		if (!to && phones.length) to = phones[0];
		loaded = true;
	}
	onMount(() => load().catch(toasts.error));

	// GSM-7 messages split every 153 characters (160 alone); anything else every 67 (70 alone).
	const gsm = $derived(/^[A-Za-z0-9 @£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞÆæßÉ!"#¤%&'()*+,\-./:;<=>?¡ÄÖÑÜ§¿äöñüà^{}\\[~\]|€]*$/.test(body));
	const parts = $derived(body.length === 0 ? 0 : gsm ? (body.length <= 160 ? 1 : Math.ceil(body.length / 153)) : body.length <= 70 ? 1 : Math.ceil(body.length / 67));

	async function send() {
		sending = true;
		try {
			await api.post(`/crm/items/${itemId}/sms`, { to, body });
			toasts.success('Text message queued');
			body = '';
			await load();
			onsent?.();
		} catch (e) {
			toasts.error(e);
		} finally {
			sending = false;
		}
	}
	const STATUS: Record<string, string> = { delivered: 'text-emerald-700', failed: 'text-red-700', undelivered: 'text-amber-700' };
</script>

{#if loaded && !enabled}
	<p class="text-sm text-slate-500" data-sms-off>Text messages are not set up. An administrator can connect Twilio in System → Text messages.</p>
{:else if loaded}
	<div class="space-y-3" data-sms-composer>
		{#if messages.length}
			<ol class="max-h-64 space-y-2 overflow-y-auto rounded-lg bg-slate-50 p-3 text-sm" data-sms-thread>
				{#each messages.slice(-20) as m (m.id)}
					<li class="flex {m.direction === 'out' ? 'justify-end' : 'justify-start'}">
						<div class="max-w-[80%] rounded-xl px-3 py-2 {m.direction === 'out' ? 'bg-brand-600 text-white' : 'bg-white text-slate-800 shadow-sm'}">
							<p class="whitespace-pre-line">{m.body}</p>
							<p class="mt-1 text-[11px] {m.direction === 'out' ? 'text-brand-100' : 'text-slate-500'}">
								{formatDateTime(m.created_at)}{#if m.direction === 'out'} · <span class={m.direction === 'out' ? '' : STATUS[m.status]} data-sms-status>{m.status}</span>{/if}
							</p>
							{#if m.error}<p class="mt-1 text-[11px] text-red-100">{m.error}</p>{/if}
						</div>
					</li>
				{/each}
			</ol>
		{/if}
		{#if phones.length}
			<label class="flex items-center gap-2 text-sm">
				<span class="text-slate-500">To</span>
				<select class="input w-56" bind:value={to} aria-label="Phone number">{#each phones as p (p)}<option value={p}>{p}</option>{/each}</select>
			</label>
			<textarea class="input" rows="3" maxlength="1600" placeholder="Write a text message…" bind:value={body} data-sms-body></textarea>
			<div class="flex items-center justify-between gap-2 text-xs text-slate-500">
				<span data-sms-count>{body.length} characters{parts > 1 ? ` · ${parts} parts` : ''}</span>
				<div class="flex gap-2">
					{#if oncancel}<button class="btn btn-ghost" onclick={oncancel}>Cancel</button>{/if}
					<button class="btn btn-primary" disabled={sending || !body.trim() || !to} onclick={send} data-sms-send>{sending ? 'Sending…' : 'Send text'}</button>
				</div>
			</div>
		{:else}
			<p class="text-sm text-slate-500">This record has no phone number. Add one to the record (or its contact or company) to text the client.</p>
		{/if}
	</div>
{/if}
