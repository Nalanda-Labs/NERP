<script lang="ts">
	// Check-in: say you're working today and where (optionally telling a chat, with where you
	// are), or that you aren't and why. Once checked in: see it, say you aren't working after
	// all, or undo it.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import { LOCATIONS, REASONS, mapLink, summary, type CheckInToday, type Location, type Reason } from '#lib/checkin.ts';
	import Modal from '#lib/components/Modal.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	let { open = $bindable(false), today = $bindable(null) }: { open?: boolean; today?: CheckInToday | null } = $props();

	let mode = $state<'working' | 'not_working'>('working');
	let location = $state<Location>('office');
	let locationText = $state('');
	let message = $state('');
	let sendMessage = $state(false);
	let chatId = $state('');
	let shareGeo = $state(false);
	let address = $state('');
	let startWorkday = $state(false);
	let reason = $state<Reason>('illness');
	let reasonText = $state('');
	let busy = $state(false);
	let cancelling = $state(false);
	let chats = $state<{ id: string; title: string; kind: string }[]>([]);
	let geo = $state<{ lat: number; lng: number } | null>(null);
	let geoState = $state<'idle' | 'finding' | 'found' | 'failed'>('idle');

	// Each time it opens: start from what was chosen last time.
	$effect(() => {
		if (!open || !today) return;
		today.checkin?.id;
		untrack(() => reset(today!));
	});
	function reset(today: CheckInToday) {
		const p = today.prefs;
		mode = 'working';
		location = p.location;
		locationText = p.location_text ?? '';
		message = p.message ?? '';
		sendMessage = p.send_message && !!p.chat_id;
		chatId = p.chat_id ?? '';
		shareGeo = today.geo_enabled && p.send_geo;
		startWorkday = p.start_workday;
		reason = 'illness';
		reasonText = '';
		cancelling = false;
		address = '';
		if (!chats.length) api.get<{ items: { id: string; title: string; kind: string }[] }>('/im/chats').then((r) => (chats = r.items)).catch(() => {});
	}
	$effect(() => {
		if (open && shareGeo && geoState === 'idle') locate();
	});

	function locate() {
		if (!navigator.geolocation) {
			geoState = 'failed';
			return;
		}
		geoState = 'finding';
		navigator.geolocation.getCurrentPosition(
			(p) => {
				geo = { lat: Math.round(p.coords.latitude * 1e6) / 1e6, lng: Math.round(p.coords.longitude * 1e6) / 1e6 };
				geoState = 'found';
			},
			() => (geoState = 'failed'),
			{ timeout: 10000, maximumAge: 300000 }
		);
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const body =
				mode === 'working'
					? {
							status: 'working', location, location_text: location === 'custom' ? locationText : null, message: message || null,
							send_message: sendMessage && !!chatId, chat_id: chatId || null, start_workday: startWorkday,
							...(shareGeo && geo ? { latitude: geo.lat, longitude: geo.lng } : {}),
							...(shareGeo && address.trim() ? { address: address.trim() } : {})
						}
					: { status: 'not_working', reason, reason_text: reason === 'custom' ? reasonText : null, message: message || null, send_message: sendMessage && !!chatId, chat_id: chatId || null };
			today = await api.post<CheckInToday>('/checkin', body);
			if (today.workday_started) window.dispatchEvent(new CustomEvent('nerp:workday'));
			toasts.success(mode === 'working' ? 'Checked in' : 'Noted: not working today');
			open = false;
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}

	async function cancelDay(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			today = await api.post<CheckInToday>(`/checkin/${today!.checkin!.id}/cancel`, { reason, reason_text: reason === 'custom' ? reasonText : null });
			cancelling = false;
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}

	async function undo() {
		if (!confirm('Undo today’s check-in? A message already posted to a chat stays there.')) return;
		try {
			today = await api.del<CheckInToday>(`/checkin/${today!.checkin!.id}`);
		} catch (err) {
			toasts.error(err);
		}
	}

	async function mute(how: 'day' | 'week' | 'forever') {
		try {
			today = await api.patch<CheckInToday>('/checkin/prefs', { mute: how });
			toasts.success(how === 'forever' ? 'No more reminders. Check in from HR → Check-in.' : how === 'day' ? 'Reminder off until tomorrow' : 'Reminder off for a week');
			open = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	const chip = (on: boolean) => `rounded-full border px-3 py-1.5 text-sm ${on ? 'border-brand-500 bg-brand-50 font-medium text-brand-700' : 'border-slate-200 text-slate-700 hover:bg-slate-50'}`;
	const k = $derived(today?.checkin ?? null);
</script>

<Modal bind:open title={k ? 'Today' : 'Check in'}>
	{#if today && !today.enabled}
		<p class="text-sm text-slate-600">Check-in is turned off for the company.</p>
	{:else if k && !cancelling}
		<div class="space-y-3" data-checked-in={k.status}>
			<p class="text-lg font-medium text-slate-900" data-summary>{summary(k)}</p>
			{#if k.message}<p class="text-sm whitespace-pre-line text-slate-600">{k.message}</p>{/if}
			<p class="text-xs text-slate-500">
				Checked in at {new Date(k.created_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}{k.chat_title ? ` · told ${k.chat_title}` : ''}
				{#if k.latitude != null && k.longitude != null} · <a class="text-brand-600 hover:underline" target="_blank" rel="noopener" href={mapLink(k.latitude, k.longitude)}>📍 {k.address ?? 'on the map'}</a>{:else if k.address} · 📍 {k.address}{/if}
			</p>
		</div>
	{:else if k && cancelling}
		<form id="ck-cancel" class="space-y-3" onsubmit={cancelDay}>
			<p class="text-sm text-slate-600">Not working today after all? Say why{k.chat_title ? `; ${k.chat_title} is told` : ''}.</p>
			<div class="flex flex-wrap gap-2" role="radiogroup" aria-label="Why">
				{#each REASONS as r (r.value)}
					<button type="button" role="radio" aria-checked={reason === r.value} class={chip(reason === r.value)} onclick={() => (reason = r.value)}>{r.label}</button>
				{/each}
			</div>
			{#if reason === 'custom'}<input id="ck-cr" class="input" required maxlength="300" placeholder="Why" bind:value={reasonText} />{/if}
		</form>
	{:else if today}
		<form id="ck-form" class="space-y-4" onsubmit={submit}>
			<div class="flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist" style="width:fit-content">
				<button type="button" role="tab" aria-selected={mode === 'working'} class="rounded-full px-4 py-1.5 font-medium {mode === 'working' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (mode = 'working')}>I'm working</button>
				<button type="button" role="tab" aria-selected={mode === 'not_working'} class="rounded-full px-4 py-1.5 font-medium {mode === 'not_working' ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => (mode = 'not_working')}>Not working today</button>
			</div>
			{#if mode === 'working'}
				<div class="flex flex-wrap gap-2" role="radiogroup" aria-label="Where">
					{#each LOCATIONS as l (l.value)}
						<button type="button" role="radio" aria-checked={location === l.value} class={chip(location === l.value)} onclick={() => (location = l.value)}>{l.icon} {l.label}</button>
					{/each}
				</div>
				{#if location === 'custom'}<input id="ck-where" class="input" required maxlength="100" placeholder="Where: at the client's, the warehouse…" bind:value={locationText} />{/if}
			{:else}
				<div class="flex flex-wrap gap-2" role="radiogroup" aria-label="Why">
					{#each REASONS as r (r.value)}
						<button type="button" role="radio" aria-checked={reason === r.value} class={chip(reason === r.value)} onclick={() => (reason = r.value)}>{r.label}</button>
					{/each}
				</div>
				{#if reason === 'custom'}<input id="ck-why" class="input" required maxlength="300" placeholder="Why" bind:value={reasonText} />{/if}
			{/if}

			<div class="space-y-2 rounded-xl border border-slate-100 p-3">
				<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={sendMessage} /> Tell a chat</label>
				{#if sendMessage}
					<select id="ck-chat" class="input" aria-label="Chat" required bind:value={chatId}>
						<option value="" disabled>Choose a chat…</option>
						{#each chats as c (c.id)}<option value={c.id}>{c.kind === 'direct' ? '👤 ' : '# '}{c.title}</option>{/each}
					</select>
					{#if mode === 'working'}<textarea id="ck-msg" class="input" rows="2" maxlength="1000" placeholder="A message (optional): plans for today…" bind:value={message}></textarea>{/if}
				{/if}
			</div>

			{#if mode === 'working'}
				{#if today.geo_enabled}
					<div class="space-y-2">
						<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={shareGeo} /> Share where I am</label>
						{#if shareGeo}
							<p class="text-xs text-slate-500" data-geo={geoState}>
								{#if geoState === 'finding'}Finding you…{:else if geoState === 'found' && geo}📍 {geo.lat.toFixed(5)}, {geo.lng.toFixed(5)} · <a class="text-brand-600 hover:underline" target="_blank" rel="noopener" href={mapLink(geo.lat, geo.lng)}>see on the map</a>{:else if geoState === 'failed'}Your browser didn't give a location; you can type an address instead. <button type="button" class="text-brand-600 hover:underline" onclick={locate}>Try again</button>{/if}
							</p>
							<input id="ck-addr" class="input" maxlength="300" placeholder="Address (optional)" bind:value={address} />
							<p class="text-xs text-slate-500">Colleagues see where you work; the exact place only your managers and the chat you tell.</p>
						{/if}
					</div>
				{/if}
				<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={startWorkday} /> Start my work day too</label>
			{/if}
		</form>
	{/if}
	{#snippet footer()}
		{#if today?.enabled && k && !cancelling}
			<button class="btn btn-ghost mr-auto text-sm" onclick={undo}>Undo</button>
			{#if k.status === 'working'}<button class="btn btn-secondary" onclick={() => ((cancelling = true), (reason = 'illness'), (reasonText = ''))}>Not working after all</button>{/if}
			<button class="btn btn-primary" onclick={() => (open = false)}>Close</button>
		{:else if today?.enabled && k}
			<button class="btn btn-secondary" onclick={() => (cancelling = false)}>Back</button>
			<button class="btn btn-primary" form="ck-cancel" disabled={busy}>Not working today</button>
		{:else if today?.enabled}
			{#if today.remind}
				<details class="relative mr-auto">
					<summary class="btn btn-ghost cursor-pointer list-none text-sm">Remind me later…</summary>
					<div class="absolute bottom-full left-0 z-10 mb-1 w-44 rounded-xl bg-white p-1 shadow-g2">
						<button class="block w-full rounded-lg px-3 py-2 text-left text-sm hover:bg-slate-50" onclick={() => mute('day')}>Not today</button>
						<button class="block w-full rounded-lg px-3 py-2 text-left text-sm hover:bg-slate-50" onclick={() => mute('week')}>Not for a week</button>
						<button class="block w-full rounded-lg px-3 py-2 text-left text-sm hover:bg-slate-50" onclick={() => mute('forever')}>Don't remind me</button>
					</div>
				</details>
			{/if}
			<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
			<button class="btn btn-primary" form="ck-form" disabled={busy || (mode === 'working' && shareGeo && geoState === 'finding')}>{mode === 'working' ? 'Check in' : 'Send'}</button>
		{:else}
			<button class="btn btn-primary" onclick={() => (open = false)}>Close</button>
		{/if}
	{/snippet}
</Modal>
