<script lang="ts">
	// A CRM record's bookings (a contact's, a deal's, or a company's contacts' and deals'),
	// and booking from the record: the client and the deal are filled in.
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let {
		itemId,
		typeCode,
		clientName,
		clientEmail,
		clientPhone,
		contactId
	}: {
		itemId: string;
		typeCode: string;
		clientName: string;
		clientEmail: string | null;
		clientPhone: string | null;
		/** The contact to book for (the record itself, or a deal's first contact). */
		contactId: string | null;
	} = $props();

	type Booking = { id: string; number: number; starts_at: string; status: string; service_name: string | null; client_name: string };
	type Resource = { id: string; name: string; is_active: boolean };
	type Service = { id: string; name: string; is_extra: boolean; is_active: boolean; duration_minutes: number };
	const STATUS: Record<string, string> = { booked: 'Booked', confirmed: 'Confirmed', came: 'Came', no_show: 'Didn’t come', cancelled: 'Cancelled', waitlist: 'Waiting list' };

	let items = $state<Booking[]>([]);
	let open = $state(false);
	let resources = $state<Resource[]>([]);
	let services = $state<Service[]>([]);
	let form = $state({ resource: '', service: '', date: new Date().toISOString().slice(0, 10), start: '' });
	let slots = $state<string[]>([]);
	let busy = $state(false);

	async function load() {
		try {
			items = (await api.get<{ items: Booking[] }>(`/crm/items/${itemId}/bookings`)).items;
		} catch {
			items = [];
		}
	}
	$effect(() => {
		itemId;
		load();
	});
	async function start() {
		open = !open;
		if (open && !resources.length) {
			const [r, s] = await Promise.all([api.get<{ items: Resource[] }>('/booking/resources'), api.get<{ items: Service[] }>('/booking/services')]);
			resources = r.items.filter((x) => x.is_active);
			services = s.items.filter((x) => !x.is_extra && x.is_active);
			form.resource = resources[0]?.id ?? '';
			form.service = services[0]?.id ?? '';
			await loadSlots();
		}
	}
	async function loadSlots() {
		slots = [];
		form.start = '';
		if (!form.resource || !form.date) return;
		const q = new URLSearchParams({ resource_id: form.resource, date: form.date });
		if (form.service) q.set('service_id', form.service);
		slots = (await api.get<{ slots: string[] }>(`/booking/slots?${q}`)).slots;
	}
	async function book(e: SubmitEvent) {
		e.preventDefault();
		if (!form.start) return;
		busy = true;
		try {
			await api.post('/booking/bookings', {
				resource_ids: [form.resource],
				service_id: form.service || null,
				starts_at: form.start,
				ends_at: form.service ? null : new Date(new Date(form.start).getTime() + 30 * 60e3).toISOString(),
				client_name: clientName,
				client_email: clientEmail,
				client_phone: clientPhone,
				contact_id: contactId,
				deal_id: typeCode === 'deal' ? itemId : null
			});
			toasts.success('Booked');
			open = false;
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

<section class="card" aria-label="Bookings" data-bookings-card>
	<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3.5">
		<h2 class="font-semibold text-slate-900">Bookings</h2>
		{#if typeCode !== 'company'}<button class="btn btn-ghost text-sm" onclick={start} data-book>{open ? 'Close' : 'Book'}</button>{/if}
	</div>
	{#if open}
		<form class="space-y-2 border-b border-slate-100 p-4 text-sm" onsubmit={book} data-book-form>
			{#if !resources.length}
				<p class="text-slate-500">Set up resources in Booking first.</p>
			{:else}
				<div class="grid grid-cols-2 gap-2">
					<select class="input" aria-label="Resource" bind:value={form.resource} onchange={loadSlots}>
						{#each resources as r (r.id)}<option value={r.id}>{r.name}</option>{/each}
					</select>
					<select class="input" aria-label="Service" bind:value={form.service} onchange={loadSlots}>
						<option value="">No service (30 minutes)</option>
						{#each services as s (s.id)}<option value={s.id}>{s.name} · {s.duration_minutes} min</option>{/each}
					</select>
				</div>
				<input class="input" type="date" aria-label="Day" bind:value={form.date} onchange={loadSlots} />
				<div class="flex flex-wrap gap-1" role="radiogroup" aria-label="Time">
					{#each slots as t (t)}
						<button type="button" class="rounded-full px-2.5 py-1 text-xs {form.start === t ? 'bg-brand-600 text-white' : 'bg-slate-100 hover:bg-slate-200'}" aria-pressed={form.start === t} onclick={() => (form.start = t)} data-slot={t}>{new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</button>
					{:else}<p class="text-xs text-slate-500">Nothing free that day.</p>{/each}
				</div>
				<p class="text-xs text-slate-500">For {clientName}{clientEmail ? ` · ${clientEmail}` : ''}{typeCode === 'deal' ? ', on this deal' : ''}.</p>
				<button class="btn btn-primary" disabled={busy || !form.start} data-book-submit>{busy ? 'Booking…' : 'Book'}</button>
			{/if}
		</form>
	{/if}
	<ul class="divide-y divide-slate-50 text-sm">
		{#each items as b (b.id)}
			<li class="flex items-center gap-2 px-5 py-2.5" data-booking={b.number}>
				<span class="flex-1">
					<a class="text-brand-700 hover:underline" href="/booking?booking={b.id}">#{b.number}</a>
					{b.service_name ?? 'Booking'}{#if typeCode === 'company'}<span class="text-slate-500"> · {b.client_name}</span>{/if}
					<span class="block text-xs text-slate-500">{formatDateTime(b.starts_at)}</span>
				</span>
				<span class="rounded-full bg-slate-100 px-2 py-0.5 text-xs" data-status={b.status}>{STATUS[b.status] ?? b.status}</span>
			</li>
		{:else}<li class="px-5 py-3 text-slate-400">No bookings.</li>{/each}
	</ul>
</section>
