<script lang="ts">
	// Booking links: a public page where people pick a time that's free in my calendar (and,
	// with colleagues added, free for all of us).
	import { api } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Person = { id: string; name: string; avatar_file_id: string | null };
	type Link = {
		id: string; owner_id: string; slug: string; title: string; description: string | null; location: string | null; duration_minutes: number;
		members: string[]; hours: [number, number, number][]; timezone: string; slot_step_minutes: number; buffer_minutes: number;
		min_notice_minutes: number; days_ahead: number; max_per_day: number | null; is_active: boolean; url: string; people: Person[];
	};
	let { open = $bindable(false) }: { open?: boolean } = $props();

	const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
	const TZ = Intl.DateTimeFormat().resolvedOptions().timeZone;
	const hm = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
	const mins = (s: string) => { const [h, m] = s.split(':').map(Number); return h * 60 + m; };

	let links = $state<Link[] | null>(null);
	let editing = $state<Link | null>(null);
	let creating = $state(false);
	let busy = $state(false);
	let form = $state(blank());
	let members = $state<Person[]>([]);

	function blank() {
		return {
			title: '', description: '', location: '', duration_minutes: 30, slot_step_minutes: 30, buffer_minutes: 0, notice_hours: 2, days_ahead: 30, max_per_day: '',
			days: DAYS.map((_, i) => ({ on: i < 5, from: '09:00', to: '17:00' })), is_active: true
		};
	}
	async function load() {
		try {
			links = (await api.get<{ items: Link[] }>('/calendar/slot-links')).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		if (open) load();
	});
	function edit(l: Link | null) {
		editing = l;
		creating = !l;
		if (!l) {
			form = blank();
			members = [];
			return;
		}
		form = {
			title: l.title, description: l.description ?? '', location: l.location ?? '', duration_minutes: l.duration_minutes, slot_step_minutes: l.slot_step_minutes,
			buffer_minutes: l.buffer_minutes, notice_hours: Math.round(l.min_notice_minutes / 6) / 10, days_ahead: l.days_ahead, max_per_day: l.max_per_day ? String(l.max_per_day) : '',
			days: DAYS.map((_, i) => { const h = l.hours.find((x) => x[0] === i + 1); return { on: !!h, from: h ? hm(h[1]) : '09:00', to: h ? hm(h[2]) : '17:00' }; }), is_active: l.is_active
		};
		members = l.people.filter((p) => p.id !== l.owner_id);
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		const f = form;
		const hours = f.days.map((d, i) => [i + 1, mins(d.from), d.to === '00:00' ? 1440 : mins(d.to), d.on] as const).filter((x) => x[3]).map(([d, a, b]) => [d, a, b]);
		const body = {
			title: f.title, description: f.description || null, location: f.location || null, duration_minutes: Number(f.duration_minutes), slot_step_minutes: Number(f.slot_step_minutes),
			buffer_minutes: Number(f.buffer_minutes), min_notice_minutes: Math.round(Number(f.notice_hours) * 60), days_ahead: Number(f.days_ahead),
			max_per_day: f.max_per_day ? Number(f.max_per_day) : null, hours, timezone: editing?.timezone ?? TZ, members: members.map((m) => m.id), is_active: f.is_active
		};
		busy = true;
		try {
			editing ? await api.patch(`/calendar/slot-links/${editing.id}`, body) : await api.post('/calendar/slot-links', body);
			editing = null;
			creating = false;
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function remove(l: Link) {
		if (!confirm(`Delete “${l.title}”? The link stops working; meetings already booked stay.`)) return;
		await api.del(`/calendar/slot-links/${l.id}`);
		editing = null;
		await load();
	}
	async function copy(l: Link) {
		try {
			await navigator.clipboard.writeText(l.url);
			toasts.success('Link copied');
		} catch {
			prompt('Copy the link:', l.url);
		}
	}
</script>

<Modal bind:open title={creating ? 'New booking link' : editing ? 'Edit booking link' : 'Booking links'}>
	{#if creating || editing}
		<form id="sl-form" class="grid gap-3 text-sm sm:grid-cols-2" onsubmit={save}>
			<div class="sm:col-span-2"><label class="label" for="sl-title">Name</label><input id="sl-title" class="input" required maxlength="200" placeholder="Intro call" bind:value={form.title} /></div>
			<div><label class="label" for="sl-dur">Length (minutes)</label><input id="sl-dur" class="input" type="number" min="5" max="480" required bind:value={form.duration_minutes} /></div>
			<div><label class="label" for="sl-step">A time every (minutes)</label><input id="sl-step" class="input" type="number" min="5" max="240" required bind:value={form.slot_step_minutes} /></div>
			<div class="sm:col-span-2">
				<span class="label">With colleagues <span class="font-normal text-slate-400">(only times when everyone is free)</span></span>
				<MultiUserPicker bind:value={members} exclude={session.user ? [session.user.id] : []} placeholder="Add colleagues…" />
			</div>
			<fieldset class="space-y-1.5 sm:col-span-2">
				<legend class="label">Hours <span class="font-normal text-slate-400">({(editing?.timezone ?? TZ).replace(/_/g, ' ')})</span></legend>
				{#each form.days as d, i (i)}
					<div class="flex items-center gap-2">
						<label class="flex w-16 items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.days[i].on} /> {DAYS[i]}</label>
						{#if d.on}
							<input class="input w-28 py-1" type="time" aria-label="{DAYS[i]} from" bind:value={form.days[i].from} />–<input class="input w-28 py-1" type="time" aria-label="{DAYS[i]} to" bind:value={form.days[i].to} />
						{:else}<span class="text-slate-400">Not offered</span>{/if}
					</div>
				{/each}
			</fieldset>
			<div><label class="label" for="sl-notice">At least (hours) ahead</label><input id="sl-notice" class="input" type="number" min="0" max="336" step="0.5" bind:value={form.notice_hours} /></div>
			<div><label class="label" for="sl-ahead">Up to (days) ahead</label><input id="sl-ahead" class="input" type="number" min="1" max="365" bind:value={form.days_ahead} /></div>
			<div><label class="label" for="sl-buf">Gap around meetings (minutes)</label><input id="sl-buf" class="input" type="number" min="0" max="240" bind:value={form.buffer_minutes} /></div>
			<div><label class="label" for="sl-max">At most a day <span class="font-normal text-slate-400">(optional)</span></label><input id="sl-max" class="input" type="number" min="1" max="50" bind:value={form.max_per_day} /></div>
			<div class="sm:col-span-2"><label class="label" for="sl-loc">Where</label><input id="sl-loc" class="input" maxlength="300" placeholder="Video call, office…" bind:value={form.location} /></div>
			<div class="sm:col-span-2"><label class="label" for="sl-desc">Shown on the page</label><textarea id="sl-desc" class="input" rows="2" bind:value={form.description}></textarea></div>
			<label class="flex items-center gap-2 sm:col-span-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.is_active} /> The link works</label>
		</form>
	{:else if links === null}
		<p class="text-sm text-slate-400">Loading…</p>
	{:else}
		<div class="space-y-3 text-sm">
			<p class="text-slate-600">Share a link where clients or colleagues pick a time that's free in your calendar. They get an invitation by email, and the meeting goes on your calendar.</p>
			{#each links as l (l.id)}
				<div class="rounded-xl border border-slate-200 p-3 {l.is_active ? '' : 'opacity-60'}" data-slot-link={l.title}>
					<div class="flex items-start gap-2">
						<div class="min-w-0 flex-1">
							<p class="font-medium text-slate-900">{l.title}{l.is_active ? '' : ' (off)'}</p>
							<p class="text-xs text-slate-500">{l.duration_minutes} min · {l.people.map((p) => p.name).join(', ')}</p>
							<a class="block truncate text-xs text-brand-600 hover:underline" href={l.url} target="_blank" rel="noopener">{l.url}</a>
						</div>
						<button class="btn btn-ghost px-2 py-1" onclick={() => copy(l)}>Copy</button>
						{#if l.owner_id === session.user?.id}<button class="btn btn-ghost px-2 py-1" onclick={() => edit(l)}>Edit</button>{/if}
					</div>
				</div>
			{:else}
				<p class="text-slate-400">No links yet.</p>
			{/each}
		</div>
	{/if}
	{#snippet footer()}
		{#if creating || editing}
			{#if editing}<button class="btn btn-ghost mr-auto hover:text-red-600" onclick={() => remove(editing!)}>Delete</button>{/if}
			<button class="btn btn-secondary" onclick={() => ((editing = null), (creating = false))}>Back</button>
			<button class="btn btn-primary" form="sl-form" disabled={busy}>Save</button>
		{:else}
			<button class="btn btn-secondary" onclick={() => (open = false)}>Close</button>
			<button class="btn btn-primary" onclick={() => edit(null)}>New link</button>
		{/if}
	{/snippet}
</Modal>
