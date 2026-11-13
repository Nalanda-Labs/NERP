<script lang="ts">
	// Setting up booking: what can be booked and when, services and extras, self-booking
	// forms, and emails to clients.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { hhmm, RESOURCE_KINDS, toMin, WEEKDAYS, type BookingForm, type Resource, type Service } from '#lib/booking.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let tab = $state<'resources' | 'services' | 'forms' | 'settings'>('resources');
	let resources = $state<Resource[]>([]);
	let services = $state<Service[]>([]);
	let forms = $state<BookingForm[]>([]);
	let settings = $state({ sms_notices: false, sms_available: false, confirm_email: true, remind_hours: 24, make_contact: true, deal_category_id: null as string | null, currency: 'USD', extra_reminders: [] as number[], follow_up_hours: 0, follow_up_text: null as string | null, no_show_text: null as string | null });
	// More reminders, as text: "2, 1" (hours before).
	let extraText = $state('');
	let pipelines = $state<{ id: string; name: string }[]>([]);

	async function load() {
		const [r, s, f, st] = await Promise.all([
			api.get<{ items: Resource[] }>('/booking/resources'),
			api.get<{ items: Service[] }>('/booking/services'),
			api.get<{ items: BookingForm[] }>('/booking/forms'),
			api.get<typeof settings>('/booking/settings')
		]);
		resources = r.items;
		services = s.items;
		forms = f.items;
		settings = st;
		extraText = (st.extra_reminders ?? []).join(', ');
	}
	onMount(async () => {
		await load();
		const cfg = await api.get<{ types: { code: string; categories: { id: string; name: string }[] }[] }>('/crm/config').catch(() => null);
		pipelines = cfg?.types.find((t) => t.code === 'deal')?.categories ?? [];
	});

	// ----- resources -----
	let resOpen = $state(false);
	type ResForm = { id: string | null; kind: Resource['kind']; name: string; user_id: string | null; user_name: string | null; description: string; capacity: number; allow_overbooking: boolean; timezone: string; slot_minutes: number; buffer_minutes: number; min_notice_minutes: number; max_days_ahead: number; is_active: boolean; hours: { weekday: number; from: string; to: string }[] };
	let rf = $state<ResForm>(blankRes());
	function blankRes(): ResForm {
		return { id: null, kind: 'person', name: '', user_id: null, user_name: null, description: '', capacity: 1, allow_overbooking: false, timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, slot_minutes: 30, buffer_minutes: 0, min_notice_minutes: 60, max_days_ahead: 60, is_active: true, hours: [1, 2, 3, 4, 5].map((d) => ({ weekday: d, from: '09:00', to: '18:00' })) };
	}
	function editRes(r: Resource | null) {
		rf = r
			? { id: r.id, kind: r.kind, name: r.name, user_id: r.user_id, user_name: r.user_name, description: r.description ?? '', capacity: r.capacity, allow_overbooking: r.allow_overbooking, timezone: r.timezone, slot_minutes: r.slot_minutes, buffer_minutes: r.buffer_minutes, min_notice_minutes: r.min_notice_minutes, max_days_ahead: r.max_days_ahead, is_active: r.is_active, hours: r.hours.map((h) => ({ weekday: h.weekday, from: hhmm(h.start_min), to: hhmm(h.end_min) })) }
			: blankRes();
		resOpen = true;
	}
	async function saveRes(e: SubmitEvent) {
		e.preventDefault();
		const body = { kind: rf.kind, name: rf.name || null, user_id: rf.kind === 'person' ? rf.user_id : null, description: rf.description || null, capacity: Number(rf.capacity), allow_overbooking: rf.allow_overbooking, timezone: rf.timezone, slot_minutes: Number(rf.slot_minutes), buffer_minutes: Number(rf.buffer_minutes), min_notice_minutes: Number(rf.min_notice_minutes), max_days_ahead: Number(rf.max_days_ahead), is_active: rf.is_active };
		try {
			const r = rf.id ? await api.patch<Resource>(`/booking/resources/${rf.id}`, body) : await api.post<Resource>('/booking/resources', body);
			await api.put(`/booking/resources/${r.id}/hours`, { hours: rf.hours.map((h) => ({ weekday: h.weekday, start_min: toMin(h.from), end_min: h.to === '24:00' ? 1440 : toMin(h.to) })) });
			resOpen = false;
			load();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeRes() {
		if (!rf.id || !confirm('Delete this resource?')) return;
		try {
			await api.del(`/booking/resources/${rf.id}`);
			resOpen = false;
			load();
		} catch (err) {
			toasts.error(err);
		}
	}

	// ----- services -----
	let svcOpen = $state(false);
	let sf = $state({ id: null as string | null, name: '', description: '', duration_minutes: 60, price: '0', is_extra: false, resource_ids: [] as string[], is_active: true });
	function editSvc(s: Service | null) {
		sf = s ? { id: s.id, name: s.name, description: s.description ?? '', duration_minutes: s.duration_minutes, price: s.price, is_extra: s.is_extra, resource_ids: [...s.resource_ids], is_active: s.is_active } : { id: null, name: '', description: '', duration_minutes: 60, price: '0', is_extra: false, resource_ids: [], is_active: true };
		svcOpen = true;
	}
	async function saveSvc(e: SubmitEvent) {
		e.preventDefault();
		const body = { ...sf, description: sf.description || null, duration_minutes: Number(sf.duration_minutes) };
		try {
			services = (sf.id ? await api.patch<{ items: Service[] }>(`/booking/services/${sf.id}`, body) : await api.post<{ items: Service[] }>('/booking/services', body)).items;
			svcOpen = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeSvc() {
		if (!sf.id || !confirm('Delete this service?')) return;
		services = (await api.del<{ items: Service[] }>(`/booking/services/${sf.id}`)).items;
		svcOpen = false;
	}

	// ----- forms -----
	let formOpen = $state(false);
	let ff = $state({ id: null as string | null, name: '', intro: '', service_ids: [] as string[], resource_ids: [] as string[], ask_phone: true, phone_required: false, choose_resource: true, waitlist: true, thanks: '', is_active: true });
	function editForm(f: BookingForm | null) {
		ff = f ? { id: f.id, name: f.name, intro: f.intro ?? '', service_ids: [...f.service_ids], resource_ids: [...f.resource_ids], ask_phone: f.ask_phone, phone_required: f.phone_required, choose_resource: f.choose_resource, waitlist: f.waitlist, thanks: f.thanks ?? '', is_active: f.is_active } : { id: null, name: '', intro: '', service_ids: [], resource_ids: [], ask_phone: true, phone_required: false, choose_resource: true, waitlist: true, thanks: '', is_active: true };
		formOpen = true;
	}
	async function saveForm(e: SubmitEvent) {
		e.preventDefault();
		const body = { ...ff, intro: ff.intro || null, thanks: ff.thanks || null };
		try {
			forms = (ff.id ? await api.patch<{ items: BookingForm[] }>(`/booking/forms/${ff.id}`, body) : await api.post<{ items: BookingForm[] }>('/booking/forms', body)).items;
			formOpen = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeForm() {
		if (!ff.id || !confirm('Delete this form? Its link stops working.')) return;
		forms = (await api.del<{ items: BookingForm[] }>(`/booking/forms/${ff.id}`)).items;
		formOpen = false;
	}
	const link = (f: BookingForm) => `${location.origin}/book/${f.slug}`;

	async function saveSettings(e: SubmitEvent) {
		e.preventDefault();
		try {
			settings = await api.put('/booking/settings', {
				...settings,
				remind_hours: Number(settings.remind_hours),
				extra_reminders: extraText.split(/[\s,;]+/).filter(Boolean).map(Number),
				follow_up_hours: Number(settings.follow_up_hours),
				follow_up_text: settings.follow_up_text ?? '',
				no_show_text: settings.no_show_text ?? ''
			});
			extraText = (settings.extra_reminders ?? []).join(', ');
			toasts.success('Saved');
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<svelte:head><title>Set up booking · NERP</title></svelte:head>

<div class="mx-auto max-w-5xl space-y-4">
	<a href="/booking" class="inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800"><Icon name="chevronRight" class="size-4 rotate-180" /> Booking</a>
	<h1 class="text-2xl font-semibold text-slate-900">Set up booking</h1>
	<div class="flex gap-1 border-b border-slate-200" role="tablist">
		{#each [['resources', 'Resources'], ['services', 'Services'], ['forms', 'Booking forms'], ['settings', 'Emails and CRM']] as [k, l] (k)}
			<button role="tab" aria-selected={tab === k} class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {tab === k ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500'}" onclick={() => (tab = k as typeof tab)}>{l}</button>
		{/each}
	</div>

	{#if tab === 'resources'}
		<div class="flex justify-end"><button class="btn btn-primary" onclick={() => editRes(null)}><Icon name="plus" class="size-4" /> Add a resource</button></div>
		<div class="card"><ul class="divide-y divide-slate-100">
			{#each resources as r (r.id)}
				<li><button class="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-slate-50 {r.is_active ? '' : 'opacity-60'}" onclick={() => editRes(r)} data-res={r.name}>
					<span class="flex-1"><span class="block font-medium text-slate-900">{r.name}</span><span class="text-xs text-slate-500">{RESOURCE_KINDS[r.kind]}{r.capacity > 1 ? ` · ${r.capacity} at once` : ''} · {r.hours.length ? [...new Set(r.hours.map((h) => WEEKDAYS[h.weekday - 1]))].join(' ') : 'no hours'} · {r.timezone}</span></span>
					{#if !r.is_active}<span class="badge bg-slate-100 text-slate-500">Off</span>{/if}
				</button></li>
			{:else}<li class="px-4 py-10 text-center text-sm text-slate-500">Add the people, rooms or equipment clients book.</li>{/each}
		</ul></div>
	{:else if tab === 'services'}
		<div class="flex justify-end"><button class="btn btn-primary" onclick={() => editSvc(null)}><Icon name="plus" class="size-4" /> Add a service</button></div>
		<div class="card"><ul class="divide-y divide-slate-100">
			{#each services as s (s.id)}
				<li><button class="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-slate-50 {s.is_active ? '' : 'opacity-60'}" onclick={() => editSvc(s)} data-svc={s.name}>
					<span class="flex-1 font-medium text-slate-900">{s.name}{#if s.is_extra} <span class="badge bg-slate-100 text-slate-600">Extra</span>{/if}</span>
					<span class="text-sm text-slate-500">{s.duration_minutes >= 1440 ? `${s.duration_minutes / 1440} d` : `${s.duration_minutes} min`} · {s.price} {settings.currency}</span>
				</button></li>
			{:else}<li class="px-4 py-10 text-center text-sm text-slate-500">No services yet.</li>{/each}
		</ul></div>
	{:else if tab === 'forms'}
		<div class="flex justify-end"><button class="btn btn-primary" onclick={() => editForm(null)}><Icon name="plus" class="size-4" /> New form</button></div>
		<div class="card"><ul class="divide-y divide-slate-100">
			{#each forms as f (f.id)}
				<li class="flex items-center gap-3 px-4 py-3" data-form={f.name}>
					<button class="flex-1 text-left" onclick={() => editForm(f)}><span class="block font-medium text-slate-900">{f.name}</span><span class="text-xs text-slate-500">{link(f)}</span></button>
					<a class="btn btn-ghost text-sm" href="/book/{f.slug}" target="_blank" rel="noopener">Open</a>
					<button class="btn btn-ghost text-sm" onclick={() => navigator.clipboard.writeText(link(f)).then(() => toasts.success('Link copied'))}>Copy link</button>
				</li>
			{:else}<li class="px-4 py-10 text-center text-sm text-slate-500">A form lets clients book themselves, from a link or your website.</li>{/each}
		</ul></div>
	{:else}
		<form class="card max-w-2xl space-y-4 p-5" onsubmit={saveSettings}>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={settings.confirm_email} /> Email clients a confirmation with a link to confirm or cancel</label>
			{#if settings.sms_available || settings.sms_notices}
				<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={settings.sms_notices} data-sms-notices /> Also text the confirmation, reminders, follow-up and no-show message to the client's phone</label>
			{/if}
			<div><label class="label" for="bs-rem">Remind clients by email (hours before; 0 for no reminder)</label><input id="bs-rem" type="number" min="0" max="168" class="input w-32" bind:value={settings.remind_hours} /></div>
			<div><label class="label" for="bs-more">More reminders (hours before, e.g. "2" or "72, 2")</label><input id="bs-more" class="input w-48" placeholder="None" bind:value={extraText} data-extra-reminders /><p class="mt-1 text-xs text-slate-500">Only the nearest reminder is sent when several fall due at once (a booking made two hours ahead gets the two-hour one).</p></div>
			<div class="grid gap-3 sm:grid-cols-[10rem_1fr]">
				<div><label class="label" for="bs-fu">Follow-up after the visit (hours; 0 for none)</label><input id="bs-fu" type="number" min="0" max="720" class="input" bind:value={settings.follow_up_hours} data-follow-up-hours /></div>
				<div><label class="label" for="bs-fut">Follow-up message</label><textarea id="bs-fut" class="input" rows="2" placeholder="Thanks for coming! Book your next visit any time." bind:value={settings.follow_up_text} data-follow-up-text></textarea></div>
			</div>
			<div><label class="label" for="bs-ns">Message to clients who didn't come (empty for none)</label><textarea id="bs-ns" class="input" rows="2" placeholder="Would you like to book another time?" bind:value={settings.no_show_text} data-no-show-text></textarea></div>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={settings.make_contact} /> Clients become CRM contacts (found again by email or phone)</label>
			<div>
				<label class="label" for="bs-cat">A deal for each booking, in</label>
				<select id="bs-cat" class="input" bind:value={settings.deal_category_id}><option value={null}>— no deals —</option>{#each pipelines as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select>
			</div>
			<div><label class="label" for="bs-cur">Currency</label><input id="bs-cur" class="input w-24 uppercase" maxlength="3" bind:value={settings.currency} /></div>
			<button class="btn btn-primary">Save</button>
		</form>
	{/if}
</div>

<Modal bind:open={resOpen} title={rf.id ? 'Resource' : 'New resource'} size="lg">
	<form id="res-form" class="space-y-3" onsubmit={saveRes}>
		<div class="grid gap-3 sm:grid-cols-2">
			<div><label class="label" for="rs-kind">Kind</label><select id="rs-kind" class="input" bind:value={rf.kind}>{#each Object.entries(RESOURCE_KINDS) as [k, l] (k)}<option value={k}>{l}</option>{/each}</select></div>
			{#if rf.kind === 'person'}
				<div><span class="label">Employee</span><UserPicker bind:value={rf.user_id} bind:label={rf.user_name} /></div>
			{/if}
			<div class="sm:col-span-2"><label class="label" for="rs-name">Name {rf.kind === 'person' ? '(the employee\'s, if empty)' : ''}</label><input id="rs-name" class="input" maxlength="200" bind:value={rf.name} /></div>
			<div><label class="label" for="rs-cap">How many at once</label><input id="rs-cap" type="number" min="1" max="1000" class="input" bind:value={rf.capacity} /></div>
			<div><label class="label" for="rs-tz">Time zone</label><input id="rs-tz" class="input" bind:value={rf.timezone} /></div>
			<div><label class="label" for="rs-slot">Clients book every (minutes)</label><input id="rs-slot" type="number" min="5" max="1440" class="input" bind:value={rf.slot_minutes} /></div>
			<div><label class="label" for="rs-buf">Break after each booking (minutes)</label><input id="rs-buf" type="number" min="0" max="480" class="input" bind:value={rf.buffer_minutes} /></div>
			<div><label class="label" for="rs-notice">Book at least … minutes ahead</label><input id="rs-notice" type="number" min="0" class="input" bind:value={rf.min_notice_minutes} /></div>
			<div><label class="label" for="rs-ahead">Up to … days ahead</label><input id="rs-ahead" type="number" min="1" max="730" class="input" bind:value={rf.max_days_ahead} /></div>
		</div>
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={rf.allow_overbooking} /> Staff may book it over capacity (overbooking)</label>
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={rf.is_active} /> Taking bookings</label>
		<div>
			<span class="label">Working hours</span>
			<ul class="space-y-1" aria-label="Working hours">
				{#each rf.hours as h, i (i)}
					<li class="flex items-center gap-2 text-sm">
						<select class="input w-24" aria-label="Day" bind:value={h.weekday}>{#each WEEKDAYS as w, d (w)}<option value={d + 1}>{w}</option>{/each}</select>
						<input type="time" class="input w-32" aria-label="From" bind:value={h.from} />–<input type="time" class="input w-32" aria-label="To" bind:value={h.to} />
						<button type="button" class="text-slate-400 hover:text-red-600" aria-label="Remove" onclick={() => rf.hours.splice(i, 1)}><Icon name="x" class="size-4" /></button>
					</li>
				{/each}
			</ul>
			<button type="button" class="mt-1 text-sm text-brand-600 hover:underline" onclick={() => rf.hours.push({ weekday: 6, from: '10:00', to: '14:00' })}>Add hours</button>
		</div>
		{#if rf.id}<button type="button" class="text-sm text-red-600 hover:underline" onclick={removeRes}>Delete resource</button>{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (resOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="res-form">Save</button>
	{/snippet}
</Modal>

<Modal bind:open={svcOpen} title={sf.id ? 'Service' : 'New service'}>
	<form id="svc-form" class="space-y-3" onsubmit={saveSvc}>
		<div><label class="label" for="sv-n">Name</label><input id="sv-n" class="input" required maxlength="200" bind:value={sf.name} /></div>
		<div><label class="label" for="sv-d">Description</label><textarea id="sv-d" class="input" rows="2" bind:value={sf.description}></textarea></div>
		<div class="grid gap-3 sm:grid-cols-2">
			<div><label class="label" for="sv-min">Minutes (1440 = a day)</label><input id="sv-min" type="number" min="5" class="input" required bind:value={sf.duration_minutes} /></div>
			<div><label class="label" for="sv-p">Price</label><input id="sv-p" class="input" inputmode="decimal" bind:value={sf.price} /></div>
		</div>
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={sf.is_extra} /> An extra (added to a booking, not booked alone)</label>
		<div>
			<span class="label">Offered by (none ticked: all)</span>
			<div class="flex flex-wrap gap-2">{#each resources as r (r.id)}<label class="flex items-center gap-1.5 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" value={r.id} bind:group={sf.resource_ids} />{r.name}</label>{/each}</div>
		</div>
		<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={sf.is_active} /> Offered</label>
		{#if sf.id}<button type="button" class="text-sm text-red-600 hover:underline" onclick={removeSvc}>Delete service</button>{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (svcOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="svc-form">Save</button>
	{/snippet}
</Modal>

<Modal bind:open={formOpen} title={ff.id ? 'Booking form' : 'New booking form'} size="lg">
	<form id="bf-form" class="space-y-3" onsubmit={saveForm}>
		<div><label class="label" for="bf-n">Name</label><input id="bf-n" class="input" required maxlength="200" bind:value={ff.name} /></div>
		<div><label class="label" for="bf-i">Introduction</label><textarea id="bf-i" class="input" rows="2" bind:value={ff.intro}></textarea></div>
		<div><span class="label">Services (none ticked: all)</span><div class="flex flex-wrap gap-2">{#each services.filter((s) => !s.is_extra) as s (s.id)}<label class="flex items-center gap-1.5 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" value={s.id} bind:group={ff.service_ids} />{s.name}</label>{/each}</div></div>
		<div><span class="label">Resources (none ticked: all)</span><div class="flex flex-wrap gap-2">{#each resources as r (r.id)}<label class="flex items-center gap-1.5 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" value={r.id} bind:group={ff.resource_ids} />{r.name}</label>{/each}</div></div>
		<div class="grid gap-2 text-sm sm:grid-cols-2">
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={ff.choose_resource} /> Clients choose who or what</label>
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={ff.ask_phone} /> Ask for a phone number</label>
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={ff.phone_required} /> The phone is required</label>
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={ff.waitlist} /> Offer the waiting list</label>
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={ff.is_active} /> Open</label>
		</div>
		<div><label class="label" for="bf-t">Message after booking</label><input id="bf-t" class="input" bind:value={ff.thanks} /></div>
		{#if ff.id}<button type="button" class="text-sm text-red-600 hover:underline" onclick={removeForm}>Delete form</button>{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (formOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="bf-form">Save</button>
	{/snippet}
</Modal>
