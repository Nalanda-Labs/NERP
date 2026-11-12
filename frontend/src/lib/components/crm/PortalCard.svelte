<script lang="ts">
	// On a contact: their client portal — on or off, whether they see their company's
	// invoices, and sending the sign-in link.
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { itemId }: { itemId: string } = $props();
	type Info = { enabled: boolean; company_wide: boolean; invited_at: string | null; last_seen_at: string | null; emails: string[] };
	let info = $state<Info | null>(null);
	let link = $state('');
	$effect(() => {
		itemId;
		api.get<Info>(`/crm/items/${itemId}/portal`).then((x) => (info = x)).catch(() => (info = null));
	});
	async function save() {
		if (!info) return;
		try {
			info = await api.put<Info>(`/crm/items/${itemId}/portal`, { enabled: info.enabled, company_wide: info.company_wide });
		} catch (e) {
			toasts.error(e);
		}
	}
	async function invite() {
		try {
			const r = await api.post<{ sent_to: string; url: string }>(`/crm/items/${itemId}/portal/invite`, {});
			link = r.url;
			toasts.success(`Sign-in link sent to ${r.sent_to}`);
			info = await api.get<Info>(`/crm/items/${itemId}/portal`);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

{#if info}
	<section class="card space-y-2 p-4 text-sm" aria-label="Client portal">
		<div class="flex items-center gap-2">
			<h2 class="font-semibold text-slate-900">Client portal</h2>
			{#if info.last_seen_at}<span class="ml-auto text-xs text-slate-500">last there {formatDateTime(info.last_seen_at)}</span>{/if}
		</div>
		<label class="flex items-center gap-2"><input type="checkbox" bind:checked={info.enabled} onchange={save} data-portal-enabled /> Can sign in to the portal</label>
		{#if info.enabled}<label class="flex items-center gap-2"><input type="checkbox" bind:checked={info.company_wide} onchange={save} /> Sees all of their company's invoices and quotes</label>{/if}
		<button class="btn btn-secondary w-full justify-center" disabled={!info.emails.length} onclick={invite} data-portal-invite>{info.emails.length ? 'Email a sign-in link' : 'Add an email address first'}</button>
		{#if link}<input class="input text-xs" readonly value={link} aria-label="Sign-in link" onfocus={(e) => (e.currentTarget as HTMLInputElement).select()} />{/if}
		{#if info.invited_at}<p class="text-xs text-slate-500">Last link sent {formatDateTime(info.invited_at)}. Clients can also ask for a link at /portal.</p>{/if}
	</section>
{/if}
