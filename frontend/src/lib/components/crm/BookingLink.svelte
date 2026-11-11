<script lang="ts">
	// "Booking link" on a CRM record: one of your slot links, made out for this record's client.
	// What they book lands here as a meeting (and on your calendar).
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';

	let { itemId }: { itemId: string } = $props();
	type SlotLink = { id: string; title: string; duration_minutes: number; is_active: boolean };
	let open = $state(false);
	let links = $state<SlotLink[] | null>(null);
	let made = $state<{ url: string; title: string } | null>(null);

	async function toggle() {
		open = !open;
		made = null;
		if (open && !links) links = (await api.get<{ items: SlotLink[] }>('/calendar/slot-links').catch(() => ({ items: [] }))).items.filter((l) => l.is_active);
	}
	async function pick(l: SlotLink) {
		try {
			made = await api.post<{ url: string; title: string }>(`/calendar/slot-links/${l.id}/for-record`, { item_id: itemId });
		} catch (e) {
			toasts.error(e);
		}
	}
	async function copy() {
		if (!made) return;
		try {
			await navigator.clipboard.writeText(made.url);
			toasts.success('Link copied');
		} catch {
			toasts.error('Copy the link from the box');
		}
	}
</script>

<div class="relative">
	<button class="btn btn-ghost px-2 py-1 text-sm" aria-haspopup="dialog" aria-expanded={open} onclick={toggle} data-booking-link><Icon name="calendar" class="size-4" /> Booking link</button>
	{#if open}
		<div class="absolute left-0 z-30 mt-1 w-80 rounded-xl border border-slate-200 bg-white p-3 text-sm shadow-lg" role="dialog" aria-label="Booking link">
			{#if made}
				<p class="mb-1 text-slate-600">Send this to the client. What they book shows up here as a meeting.</p>
				<input class="input text-xs" readonly value={made.url} onfocus={(e) => e.currentTarget.select()} aria-label="Booking link for this record" data-booking-url />
				<div class="mt-2 flex gap-2"><button class="btn btn-primary" onclick={copy}>Copy</button><button class="btn btn-ghost" onclick={() => (made = null)}>Another link</button></div>
			{:else if links === null}
				<p class="text-slate-400">Loading…</p>
			{:else if !links.length}
				<p class="text-slate-600">You have no booking links yet. <a class="text-brand-700 hover:underline" href="/calendar?slots=1">Make one in the calendar</a>.</p>
			{:else}
				<p class="mb-1 text-slate-500">Which of your booking links?</p>
				<ul class="divide-y divide-slate-50">
					{#each links as l (l.id)}
						<li><button class="flex w-full justify-between gap-2 px-1 py-2 text-left hover:bg-slate-50" onclick={() => pick(l)}><span class="truncate text-slate-800">{l.title}</span><span class="text-slate-400">{l.duration_minutes} min</span></button></li>
					{/each}
				</ul>
			{/if}
		</div>
	{/if}
</div>
