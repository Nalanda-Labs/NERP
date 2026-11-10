<script lang="ts">
	// A new booking, or one booking's details: client, time, resources, status, CRM links.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import { BOOKING_STATUS, type Booking, type BookingStatus, type Resource, type Service } from '#lib/booking.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Modal from '../Modal.svelte';

	let {
		open = $bindable(false),
		booking = null,
		start = null,
		resourceId = null,
		resources,
		services,
		onchanged
	}: {
		open?: boolean;
		/** Show this booking; otherwise make a new one. */
		booking?: Booking | null;
		start?: Date | null;
		resourceId?: string | null;
		resources: Resource[];
		services: Service[];
		onchanged: () => void;
	} = $props();

	const local = (d: Date) => new Date(+d - d.getTimezoneOffset() * 60e3).toISOString().slice(0, 16);
	let form = $state({ service_id: '', extra_ids: [] as string[], resource_ids: [] as string[], start: '', minutes: 60, client_name: '', client_email: '', client_phone: '', note: '', overbook: false, waitlist: false });
	let move = $state({ start: '', minutes: 60 });
	$effect(() => {
		if (!open) return;
		untrack(() => {
			if (booking) {
				move = { start: local(new Date(booking.starts_at)), minutes: Math.round((Date.parse(booking.ends_at) - Date.parse(booking.starts_at)) / 60e3) };
			} else {
				const s = start ?? new Date(Math.ceil(Date.now() / 1800e3) * 1800e3);
				form = { service_id: '', extra_ids: [], resource_ids: resourceId ? [resourceId] : [], start: local(s), minutes: 60, client_name: '', client_email: '', client_phone: '', note: '', overbook: false, waitlist: false };
			}
		});
	});
	const main = $derived(services.filter((s) => !s.is_extra && s.is_active));
	const extras = $derived(services.filter((s) => s.is_extra && s.is_active));
	$effect(() => {
		const s = services.find((x) => x.id === form.service_id);
		if (s) untrack(() => (form.minutes = s.duration_minutes));
	});

	async function create(e: SubmitEvent) {
		e.preventDefault();
		const s = new Date(form.start);
		try {
			await api.post('/booking/bookings', {
				resource_ids: form.resource_ids,
				service_id: form.service_id || null,
				extra_ids: form.extra_ids,
				starts_at: s.toISOString(),
				ends_at: new Date(+s + form.minutes * 60e3).toISOString(),
				client_name: form.client_name,
				client_email: form.client_email || null,
				client_phone: form.client_phone || null,
				note: form.note || null,
				overbook: form.overbook,
				waitlist: form.waitlist
			});
			open = false;
			toasts.success(form.waitlist ? 'On the waiting list' : 'Booked');
			onchanged();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function status(s: BookingStatus) {
		try {
			booking = await api.post<Booking>(`/booking/bookings/${booking!.id}/status`, { status: s });
			onchanged();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function saveMove(e: SubmitEvent) {
		e.preventDefault();
		const s = new Date(move.start);
		try {
			booking = await api.patch<Booking>(`/booking/bookings/${booking!.id}`, { starts_at: s.toISOString(), ends_at: new Date(+s + move.minutes * 60e3).toISOString() });
			toasts.success('Moved');
			onchanged();
		} catch (err) {
			toasts.error(err);
		}
	}
	const names = (ids: string[]) => ids.map((i) => resources.find((r) => r.id === i)?.name ?? '—').join(', ');
	const overbookable = $derived(form.resource_ids.some((r) => resources.find((x) => x.id === r)?.allow_overbooking));
</script>

<Modal bind:open title={booking ? `Booking #${booking.number}` : 'New booking'} size="lg">
	{#if booking}
		{@const b = booking}
		<div class="space-y-4" data-booking={b.number}>
			<div class="flex flex-wrap items-center gap-2">
				<span class="badge {BOOKING_STATUS[b.status].badge}" data-status={b.status}>{BOOKING_STATUS[b.status].label}</span>
				{#if b.overbooked}<span class="badge bg-amber-50 text-amber-800">Overbooked</span>{/if}
				<span class="text-sm text-slate-600">{formatDateTime(b.starts_at)} – {new Date(b.ends_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
			</div>
			<dl class="grid gap-x-4 gap-y-2 text-sm sm:grid-cols-[8rem_1fr]">
				<dt class="text-slate-500">Client</dt><dd class="text-slate-900">{b.client_name}{b.client_phone ? ` · ${b.client_phone}` : ''}{b.client_email ? ` · ${b.client_email}` : ''}</dd>
				<dt class="text-slate-500">With</dt><dd>{names(b.resource_ids)}</dd>
				{#if b.service_name}<dt class="text-slate-500">Service</dt><dd>{b.service_name}{b.extra_ids.length ? ` + ${b.extra_ids.map((e) => services.find((s) => s.id === e)?.name).join(', ')}` : ''}</dd>{/if}
				<dt class="text-slate-500">Price</dt><dd>{b.price}</dd>
				{#if b.note}<dt class="text-slate-500">Note</dt><dd class="whitespace-pre-line">{b.note}</dd>{/if}
				<dt class="text-slate-500">Made</dt><dd>{b.source === 'form' ? 'by the client, on a form' : (b.created_by_name ?? '')} · {formatDateTime(b.created_at)}</dd>
				{#if b.contact_id || b.deal_id}
					<dt class="text-slate-500">CRM</dt>
					<dd class="flex gap-3">{#if b.contact_id}<a class="text-brand-600 hover:underline" href="/crm/contact/{b.contact_id}">Contact</a>{/if}{#if b.deal_id}<a class="text-brand-600 hover:underline" href="/crm/deal/{b.deal_id}">Deal</a>{/if}</dd>
				{/if}
			</dl>
			{#if b.status !== 'cancelled'}
				<div class="flex flex-wrap gap-2 border-t border-slate-100 pt-3">
					{#if b.status === 'waitlist'}<button class="btn btn-primary" onclick={() => status('booked')}>Place it</button>{/if}
					{#if b.status === 'booked'}<button class="btn btn-secondary" onclick={() => status('confirmed')}>Confirmed</button>{/if}
					{#if b.status === 'booked' || b.status === 'confirmed'}
						<button class="btn btn-secondary" onclick={() => status('came')}>Came</button>
						<button class="btn btn-secondary" onclick={() => status('no_show')}>No-show</button>
					{/if}
					<button class="btn btn-ghost ml-auto text-red-600" onclick={() => confirm('Cancel this booking?') && status('cancelled')}>Cancel booking</button>
				</div>
				<form class="flex flex-wrap items-end gap-2 border-t border-slate-100 pt-3" onsubmit={saveMove}>
					<div><label class="label" for="bk-mv">Move to</label><input id="bk-mv" type="datetime-local" class="input" required bind:value={move.start} /></div>
					<div><label class="label" for="bk-mm">Minutes</label><input id="bk-mm" type="number" min="5" class="input w-24" bind:value={move.minutes} /></div>
					<button class="btn btn-secondary">Move</button>
				</form>
			{/if}
		</div>
	{:else}
		<form id="booking-form" class="space-y-3" onsubmit={create}>
			<div class="grid gap-3 sm:grid-cols-2">
				<div>
					<label class="label" for="bk-svc">Service</label>
					<select id="bk-svc" class="input" bind:value={form.service_id}>
						<option value="">— none —</option>
						{#each main as s (s.id)}<option value={s.id}>{s.name} · {s.duration_minutes} min · {s.price}</option>{/each}
					</select>
				</div>
				<div>
					<span class="label">With</span>
					<div class="flex flex-wrap gap-2" role="group" aria-label="Resources">
						{#each resources.filter((r) => r.is_active) as r (r.id)}
							<label class="flex items-center gap-1.5 rounded-lg border px-2 py-1 text-sm {form.resource_ids.includes(r.id) ? 'border-brand-500 bg-brand-50' : 'border-slate-200'}">
								<input type="checkbox" class="rounded border-slate-300 text-brand-600" value={r.id} bind:group={form.resource_ids} />{r.name}
							</label>
						{/each}
					</div>
				</div>
				<div><label class="label" for="bk-st">Starts</label><input id="bk-st" type="datetime-local" class="input" required bind:value={form.start} /></div>
				<div><label class="label" for="bk-min">Minutes</label><input id="bk-min" type="number" min="5" class="input" required bind:value={form.minutes} /></div>
				<div><label class="label" for="bk-name">Client</label><input id="bk-name" class="input" required maxlength="200" bind:value={form.client_name} /></div>
				<div><label class="label" for="bk-phone">Phone</label><input id="bk-phone" class="input" bind:value={form.client_phone} /></div>
				<div class="sm:col-span-2"><label class="label" for="bk-email">Email <span class="font-normal text-slate-400">— for the confirmation and reminder</span></label><input id="bk-email" type="email" class="input" bind:value={form.client_email} /></div>
			</div>
			{#if extras.length}
				<div class="flex flex-wrap gap-3 text-sm" role="group" aria-label="Extras">
					<span class="label w-full">Extras</span>
					{#each extras as x (x.id)}<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" value={x.id} bind:group={form.extra_ids} />{x.name} ({x.price})</label>{/each}
				</div>
			{/if}
			<div><label class="label" for="bk-note">Note</label><textarea id="bk-note" class="input" rows="2" bind:value={form.note}></textarea></div>
			<div class="flex flex-wrap gap-4 text-sm">
				<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.waitlist} /> Put on the waiting list</label>
				{#if overbookable}<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.overbook} /> Book even if full (overbooking)</label>{/if}
			</div>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Close</button>
		{#if !booking}<button class="btn btn-primary" form="booking-form" disabled={!form.resource_ids.length}>Book</button>{/if}
	{/snippet}
</Modal>
