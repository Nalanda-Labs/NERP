<script module lang="ts">
	export type CalPerson = { id: string; name: string; avatar_file_id: string | null; status?: string | null; owner?: boolean };
	export type CalEvent = {
		id: string;
		owner_id: string;
		title: string;
		description: string | null;
		location: string | null;
		starts_at: string;
		ends_at: string;
		all_day: boolean;
		timezone: string;
		recurrence: { freq: 'daily' | 'weekly' | 'monthly'; interval: number; weekdays: number[]; month_day: number | null; ends_on: string | null; max_runs: number | null } | null;
		repeat: string | null;
		color: string | null;
		remind_minutes: number | null;
		is_private: boolean;
		item: { id: string; code: string; title: string } | null;
		people: CalPerson[];
		can_edit: boolean;
		room: { id: string; name: string } | null;
		email_guests: EmailGuest[];
		sequence: number;
	};
	export type EmailGuest = { id?: string; email: string; name: string | null; status?: string };
</script>

<script lang="ts">
	// Creating, editing and answering one calendar event.
	import { api } from '#lib/api.ts';
	import ChatCallButtons from '#lib/components/ChatCallButtons.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import CrmPicker from '#lib/components/crm/CrmPicker.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let {
		open = $bindable(false),
		eventId = null,
		occurrence = null,
		startAt = null,
		onchanged,
		projectId = null
	}: { open?: boolean; eventId?: string | null; occurrence?: string | null; startAt?: Date | null; onchanged?: () => void; projectId?: string | null } = $props();

	const TZ = Intl.DateTimeFormat().resolvedOptions().timeZone;
	const pad = (n: number) => String(n).padStart(2, '0');
	const day = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
	const clock = (d: Date) => `${pad(d.getHours())}:${pad(d.getMinutes())}`;
	const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
	const COLORS = ['', '#1a73e8', '#188038', '#e37400', '#d93025', '#9334e6', '#12828c'];
	const REMINDERS: [string, string][] = [['', 'No reminder'], ['0', 'At the start'], ['10', '10 minutes before'], ['30', '30 minutes before'], ['60', '1 hour before'], ['1440', '1 day before']];

	let ev = $state<CalEvent | null>(null);
	let busy = $state(false);
	let form = $state(blank(new Date()));
	let guests = $state<{ id: string; name: string; avatar_file_id: string | null }[]>([]);
	let item = $state<{ id: string; title: string } | null>(null);
	// Guests from outside, by email.
	let emailGuests = $state<EmailGuest[]>([]);
	let guestText = $state('');
	// Meeting rooms with whether they're free at the chosen time.
	let roomId = $state('');
	let rooms = $state<{ id: string; name: string; capacity: number; free: boolean }[]>([]);
	// Whose calendar a new event goes on: mine, or a colleague's who lets me change theirs.
	let calendarFor = $state('');
	let editableCalendars = $state<{ id: string; name: string }[]>([]);

	function blank(at: Date) {
		const s = new Date(at);
		const e = new Date(s.getTime() + 60 * 60 * 1000);
		return { title: '', all_day: false, sday: day(s), stime: clock(s), eday: day(e), etime: clock(e), location: '', description: '', freq: '', interval: 1, weekdays: [((s.getDay() + 6) % 7) + 1], ends_on: '', remind: '10', is_private: false, color: '' };
	}
	$effect(() => {
		if (!open) return;
		ev = null;
		item = null;
		guests = [];
		emailGuests = [];
		guestText = '';
		roomId = '';
		calendarFor = '';
		if (!eventId) {
			api.get<{ items: { id: string; name: string; level: string }[] }>('/calendar/shared-with-me').then((r) => (editableCalendars = r.items.filter((x) => x.level === 'edit'))).catch(() => (editableCalendars = []));
		}
		if (eventId) {
			api.get<CalEvent>(`/calendar/events/${eventId}`).then((e) => {
				ev = e;
				const s = new Date(e.starts_at);
				const en = new Date(e.all_day ? new Date(e.ends_at).getTime() - 86400000 : e.ends_at);
				const d = (x: Date) => (e.all_day ? x.toISOString().slice(0, 10) : day(x));
				form = {
					title: e.title, all_day: e.all_day, sday: d(s), stime: clock(s), eday: d(en), etime: clock(en), location: e.location ?? '', description: e.description ?? '',
					freq: e.recurrence?.freq ?? '', interval: e.recurrence?.interval ?? 1, weekdays: e.recurrence?.weekdays ?? [((s.getDay() + 6) % 7) + 1], ends_on: e.recurrence?.ends_on ?? '',
					remind: e.remind_minutes === null ? '' : String(e.remind_minutes), is_private: e.is_private, color: e.color ?? ''
				};
				guests = e.people.filter((p) => !p.owner);
				item = e.item;
				emailGuests = e.email_guests;
				roomId = e.room?.id ?? '';
			}).catch((err) => { toasts.error(err); open = false; });
		} else {
			form = blank(startAt ?? new Date(Math.ceil(Date.now() / 1800000) * 1800000));
		}
	});
	const editable = $derived(!eventId || !!ev?.can_edit);
	// The organiser and guests share the event's chat and its call.
	const onIt = $derived(!!ev?.people.some((p) => p.id === session.user?.id));
	const eventChat = async () => (await api.post<{ chat_id: string }>(`/calendar/events/${ev!.id}/chat`)).chat_id;
	const myAnswer = $derived(ev?.people.find((p) => p.id === session.user?.id && !p.owner)?.status ?? null);

	// Rooms free at the chosen time (the event's own time doesn't count against it).
	const span = $derived.by(() => {
		const f = form;
		if (!f.sday || !f.eday) return null;
		const s = f.all_day ? new Date(`${f.sday}T00:00:00Z`) : new Date(`${f.sday}T${f.stime || '00:00'}`);
		const e = f.all_day ? new Date(new Date(`${f.eday}T00:00:00Z`).getTime() + 86400000) : new Date(`${f.eday}T${f.etime || '00:00'}`);
		return isNaN(+s) || isNaN(+e) || e <= s ? null : { from: s.toISOString(), to: e.toISOString() };
	});
	let roomSeq = 0;
	$effect(() => {
		if (!open || !span || !editable) return;
		const mine = ++roomSeq;
		const q = new URLSearchParams({ from: span.from, to: span.to });
		if (eventId) q.set('except', eventId);
		api.get<{ items: typeof rooms }>(`/calendar/rooms?${q}`).then((r) => { if (mine === roomSeq) rooms = r.items; }).catch(() => (rooms = []));
	});
	function addGuests() {
		// Addresses separated by commas, semicolons or new lines; "Name <address>" works too.
		const parts = guestText.split(/[,;\n]+/).map((x) => x.trim()).filter(Boolean);
		const bad: string[] = [];
		for (const p of parts) {
			const m = p.match(/^(.*?)<([^>]+)>$/);
			const email = (m ? m[2] : p).trim().toLowerCase();
			const name = m ? m[1].trim().replace(/^"|"$/g, '') || null : null;
			if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) { bad.push(p); continue; }
			if (!emailGuests.some((g) => g.email === email)) emailGuests = [...emailGuests, { email, name }];
		}
		guestText = bad.join(', ');
		if (bad.length) toasts.error(`Not an email address: ${bad.join(', ')}`);
	}

	async function save(e: SubmitEvent) {
		e.preventDefault();
		if (guestText.trim()) addGuests();
		if (guestText.trim()) return;
		const f = form;
		const body: Record<string, unknown> = {
			title: f.title,
			all_day: f.all_day,
			start: f.all_day ? f.sday : new Date(`${f.sday}T${f.stime}`).toISOString(),
			end: f.all_day ? f.eday : new Date(`${f.eday}T${f.etime}`).toISOString(),
			timezone: TZ,
			location: f.location || null,
			description: f.description || null,
			repeat: f.freq ? { freq: f.freq, interval: Number(f.interval) || 1, weekdays: f.freq === 'weekly' ? f.weekdays : [], month_day: f.freq === 'monthly' ? new Date(`${f.sday}T00:00`).getDate() : null, ends_on: f.ends_on || null } : null,
			remind_minutes: f.remind === '' ? null : Number(f.remind),
			is_private: f.is_private,
			color: f.color || null,
			item_id: item?.id ?? null,
			attendees: guests.map((g) => g.id),
			room_id: roomId || null,
			email_guests: emailGuests.map((g) => ({ email: g.email, name: g.name }))
		};
		if (!eventId && calendarFor) body.calendar_id = calendarFor;
		// A new event on a workgroup's calendar.
		if (!eventId && projectId) body.project_id = projectId;
		busy = true;
		try {
			eventId ? await api.patch(`/calendar/events/${eventId}`, body) : await api.post('/calendar/events', body);
			open = false;
			onchanged?.();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function remove(all: boolean) {
		if (!ev) return;
		if (!confirm(all ? `Delete “${ev.title}”${ev.recurrence ? ' and all its repeats' : ''}? Guests are told.` : 'Delete only this occurrence? Guests are told.')) return;
		await api.del(`/calendar/events/${ev.id}${!all && occurrence ? `?occurrence=${encodeURIComponent(occurrence)}` : ''}`);
		open = false;
		onchanged?.();
	}
	async function answer(status: string) {
		ev = await api.post<CalEvent>(`/calendar/events/${ev!.id}/respond`, { status });
		onchanged?.();
	}
	const STATUS: Record<string, string> = { pending: 'not answered', accepted: 'going', declined: 'not going', tentative: 'maybe' };
	const fmt = (iso: string, allDay: boolean) => (allDay ? new Date(iso).toLocaleDateString(undefined, { timeZone: 'UTC', weekday: 'short', day: 'numeric', month: 'short' }) : new Date(iso).toLocaleString(undefined, { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }));
</script>

<Modal bind:open title={!eventId ? 'New event' : editable ? 'Edit event' : (ev?.title ?? '')}>
	{#if eventId && !ev}
		<p class="text-sm text-slate-400">Loading…</p>
	{:else if !editable && ev}
		<div class="space-y-3 text-sm" data-event-view>
			<p class="text-slate-800">{fmt(ev.starts_at, ev.all_day)} – {fmt(ev.all_day ? new Date(new Date(ev.ends_at).getTime() - 86400000).toISOString() : ev.ends_at, ev.all_day)}</p>
			{#if ev.repeat}<p class="text-slate-600">{ev.repeat}</p>{/if}
			{#if ev.room || ev.location}<p class="text-slate-600">📍 {[ev.room?.name, ev.location].filter(Boolean).join(', ')}</p>{/if}
			{#if ev.description}<p class="whitespace-pre-line text-slate-700">{ev.description}</p>{/if}
			{#if ev.item}<p><a class="text-brand-600 hover:underline" href="/crm/{ev.item.code}/{ev.item.id}">{ev.item.title}</a></p>{/if}
			<ul class="space-y-1">
				{#each ev.people as p (p.id)}<li>{p.name} <span class="text-xs text-slate-500">{p.owner ? 'organiser' : STATUS[p.status ?? 'pending']}</span></li>{/each}
				{#each ev.email_guests as g (g.email)}<li>{g.name ?? g.email} <span class="text-xs text-slate-500">by email · {STATUS[g.status ?? 'pending']}</span></li>{/each}
			</ul>
			{#if myAnswer}
				<div class="flex gap-2 pt-2" role="group" aria-label="Your answer">
					{#each [['accepted', 'Going'], ['tentative', 'Maybe'], ['declined', 'Not going']] as [v, l] (v)}
						<button class="btn {myAnswer === v ? 'btn-primary' : 'btn-secondary'}" aria-pressed={myAnswer === v} onclick={() => answer(v)}>{l}</button>
					{/each}
				</div>
			{/if}
		</div>
	{:else}
		<form id="ev-form" class="grid gap-3 sm:grid-cols-2" onsubmit={save}>
			<div class="sm:col-span-2"><label class="label" for="ev-title">Title</label><input id="ev-title" class="input" required maxlength="300" bind:value={form.title} /></div>
			{#if !eventId && editableCalendars.length && !projectId}
				<div class="sm:col-span-2">
					<label class="label" for="ev-cal">Calendar</label>
					<select id="ev-cal" class="input" bind:value={calendarFor}>
						<option value="">Mine</option>
						{#each editableCalendars as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
					</select>
				</div>
			{/if}
			<label class="flex items-center gap-2 text-sm sm:col-span-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.all_day} /> All day</label>
			<div><label class="label" for="ev-sday">Starts</label><div class="flex gap-2"><input id="ev-sday" class="input" type="date" required bind:value={form.sday} onchange={() => form.eday < form.sday && (form.eday = form.sday)} />{#if !form.all_day}<input class="input w-28" type="time" required aria-label="Start time" bind:value={form.stime} />{/if}</div></div>
			<div><label class="label" for="ev-eday">Ends</label><div class="flex gap-2"><input id="ev-eday" class="input" type="date" required bind:value={form.eday} />{#if !form.all_day}<input class="input w-28" type="time" required aria-label="End time" bind:value={form.etime} />{/if}</div></div>
			<div>
				<label class="label" for="ev-rep">Repeat</label>
				<select id="ev-rep" class="input" bind:value={form.freq}><option value="">Doesn't repeat</option><option value="daily">Daily</option><option value="weekly">Weekly</option><option value="monthly">Monthly</option></select>
			</div>
			{#if form.freq}
				<div><label class="label" for="ev-until">Until <span class="font-normal text-slate-400">(optional)</span></label><input id="ev-until" class="input" type="date" bind:value={form.ends_on} /></div>
				{#if form.freq === 'weekly'}
					<div class="flex flex-wrap gap-1.5 sm:col-span-2" role="group" aria-label="Days">
						{#each WEEKDAYS as w, i (w)}
							<button type="button" class="rounded-lg border px-2.5 py-1 text-sm {form.weekdays.includes(i + 1) ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-600'}" aria-pressed={form.weekdays.includes(i + 1)} onclick={() => (form.weekdays = form.weekdays.includes(i + 1) ? form.weekdays.filter((x) => x !== i + 1) : [...form.weekdays, i + 1].sort())}>{w}</button>
						{/each}
					</div>
				{/if}
			{/if}
			{#if rooms.length || roomId}
				<div>
					<label class="label" for="ev-room">Meeting room</label>
					<select id="ev-room" class="input" bind:value={roomId}>
						<option value="">No room</option>
						{#each rooms as r (r.id)}<option value={r.id} disabled={!r.free && r.id !== roomId}>{r.name}{r.capacity > 1 ? ` (${r.capacity})` : ''}{r.free ? '' : ' — taken'}</option>{/each}
						{#if roomId && !rooms.some((r) => r.id === roomId)}<option value={roomId}>{ev?.room?.name ?? 'Room'}</option>{/if}
					</select>
					{#if roomId && rooms.find((r) => r.id === roomId)?.free === false}<p class="mt-1 text-xs text-red-600">Taken at this time.</p>{/if}
				</div>
			{/if}
			<div class={rooms.length || roomId ? '' : 'sm:col-span-2'}><label class="label" for="ev-loc">{rooms.length || roomId ? 'Other place' : 'Where'}</label><input id="ev-loc" class="input" maxlength="300" bind:value={form.location} /></div>
			<div class="sm:col-span-2"><span class="label">Guests</span><MultiUserPicker bind:value={guests} placeholder="Invite colleagues…" /></div>
			<div class="sm:col-span-2">
				<label class="label" for="ev-email">Guests by email <span class="font-normal text-slate-400">(they get an invitation to accept or decline)</span></label>
				{#if emailGuests.length}
					<ul class="mb-1.5 flex flex-wrap gap-1.5" data-email-guests>
						{#each emailGuests as g (g.email)}
							<li class="flex items-center gap-1 rounded-full bg-slate-100 py-0.5 pr-1 pl-2.5 text-sm">
								{g.name ? `${g.name} <${g.email}>` : g.email}
								{#if g.status && g.status !== 'pending'}<span class="text-xs text-slate-500">· {STATUS[g.status]}</span>{/if}
								<button type="button" class="rounded-full px-1 text-slate-400 hover:text-red-600" aria-label="Remove {g.email}" onclick={() => (emailGuests = emailGuests.filter((x) => x.email !== g.email))}>×</button>
							</li>
						{/each}
					</ul>
				{/if}
				<div class="flex gap-2">
					<input id="ev-email" class="input" type="text" placeholder="name@example.com, …" bind:value={guestText} onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addGuests(); } }} />
					<button type="button" class="btn btn-secondary" onclick={addGuests} disabled={!guestText.trim()}>Add</button>
				</div>
			</div>
			<div class="sm:col-span-2"><label class="label" for="ev-desc">Notes</label><textarea id="ev-desc" class="input" rows="2" bind:value={form.description}></textarea></div>
			<div><label class="label" for="ev-rem">Reminder</label><select id="ev-rem" class="input" bind:value={form.remind}>{#each REMINDERS as [v, l] (v)}<option value={v}>{l}</option>{/each}</select></div>
			<div>
				<span class="label">Colour</span>
				<div class="flex gap-1.5" role="group" aria-label="Colour">
					{#each COLORS as c (c)}<button type="button" class="size-7 rounded-full border-2 {form.color === c ? 'border-slate-900' : 'border-transparent'}" style="background:{c || '#cbd5e1'}" aria-label={c || 'Default colour'} aria-pressed={form.color === c} onclick={() => (form.color = c)}></button>{/each}
				</div>
			</div>
			<div class="sm:col-span-2">
				<span class="label">About a CRM record</span>
				{#if item}<p class="text-sm">{item.title} <button type="button" class="ml-2 text-slate-400 hover:text-red-600" onclick={() => (item = null)}>remove</button></p>
				{:else}<CrmPicker types={['deal', 'lead', 'contact', 'company']} placeholder="Find a deal, contact…" onpick={(h) => (item = { id: h.id, title: h.title })} />{/if}
			</div>
			<label class="flex items-center gap-2 text-sm sm:col-span-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.is_private} /> Private: colleagues see only “Busy”</label>
		</form>
	{/if}
	{#snippet footer()}
		{#if ev?.can_edit}
			<button class="btn btn-ghost mr-auto hover:text-red-600" onclick={() => remove(true)}>Delete</button>
			{#if ev.recurrence && occurrence}<button class="btn btn-ghost hover:text-red-600" onclick={() => remove(false)}>Delete this one</button>{/if}
		{/if}
		{#if ev && eventId && onIt}<div class="flex gap-2 {ev.can_edit ? '' : 'mr-auto'}" data-event-chat><ChatCallButtons chat={eventChat} call={ev.people.length > 1} /></div>{/if}
		<button class="btn btn-secondary" onclick={() => (open = false)}>{editable ? 'Cancel' : 'Close'}</button>
		{#if editable}<button class="btn btn-primary" form="ev-form" disabled={busy}>Save</button>{/if}
	{/snippet}
</Modal>
