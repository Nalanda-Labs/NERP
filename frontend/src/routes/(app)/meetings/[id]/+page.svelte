<script lang="ts">
	// One meeting: its agenda (topics, presenters' reports, decisions, tasks), its people and
	// attendance, the minutes, and moving it through prepare → in progress → closed.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, downloadFile } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { meetingTime, ROLE_LABEL, STATE_LABEL, STATE_TONE, tree, type AgendaItem, type Meeting } from '#lib/meetings.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	const id = $derived(page.params.id!);
	let m = $state<Meeting | null>(null);
	let missing = $state(false);
	const me = $derived(session.user?.id ?? '');
	const running = $derived(m?.state === 'in_progress');
	const closed = $derived(m?.state === 'closed');
	const topics = $derived(m ? tree(m.agenda) : []);

	async function load() {
		try {
			m = await api.get<Meeting>(`/meetings/${id}`);
			minutes = m.minutes ?? '';
		} catch {
			missing = true;
		}
	}
	// Loads on arrival and again when moving to another meeting of the series.
	$effect(() => {
		void id;
		load();
	});

	async function call<T = Meeting>(fn: () => Promise<T>, done?: string) {
		try {
			const r = await fn();
			if (r && typeof r === 'object' && 'agenda' in (r as object)) m = r as unknown as Meeting;
			if (done) toasts.success(done);
			return r;
		} catch (e) {
			toasts.error(e);
		}
	}
	const post = (path: string, body?: Record<string, unknown>, done?: string) => call(() => api.post<Meeting>(path, body), done);
	const patchItem = (aid: string, body: Record<string, unknown>) => call(() => api.patch<Meeting>(`/meetings/agenda/${aid}`, body));

	// ---- the meeting itself ----
	async function close() {
		if (!confirm('Close the meeting and send the minutes to everyone?')) return;
		await post(`/meetings/${id}/close`, undefined, 'Minutes sent');
	}
	async function remove() {
		if (!confirm(`Cancel and delete “${m?.title}”? It is removed from everyone's calendar.`)) return;
		try {
			await api.del(`/meetings/${id}`);
			await goto('/meetings');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function chat() {
		if (!m?.event_id) return;
		try {
			const c = await api.post<{ chat_id: string }>(`/calendar/events/${m.event_id}/chat`);
			await goto(`/messenger?chat=${c.chat_id}`);
		} catch (e) {
			toasts.error(e);
		}
	}

	let editOpen = $state(false);
	let ed = $state({ title: '', start: '', duration: 60, place: '', description: '' });
	const pad = (n: number) => String(n).padStart(2, '0');
	const localInput = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
	function startEdit() {
		if (!m) return;
		ed = { title: m.title, start: localInput(new Date(m.starts_at)), duration: m.duration_minutes, place: m.place ?? '', description: m.description ?? '' };
		editOpen = true;
	}
	async function saveEdit(e: SubmitEvent) {
		e.preventDefault();
		const r = await call(() => api.patch<Meeting>(`/meetings/${id}`, { title: ed.title, starts_at: new Date(ed.start).toISOString(), duration_minutes: Number(ed.duration), place: ed.place || null, description: ed.description || null }), 'Saved');
		if (r) editOpen = false;
	}

	let peopleOpen = $state(false);
	let picked = $state<PickedUser[]>([]);
	let keeperId = $state('');
	function startPeople() {
		if (!m) return;
		picked = m.members.filter((x) => x.role !== 'owner').map((x) => ({ id: x.id, name: x.name, avatar_file_id: x.avatar_file_id }));
		keeperId = m.members.find((x) => x.role === 'keeper')?.id ?? '';
		peopleOpen = true;
	}
	async function savePeople() {
		const list = picked.map((p) => ({ user_id: p.id, role: p.id === keeperId ? 'keeper' : 'member' }));
		const r = await call(() => api.put<Meeting>(`/meetings/${id}/members`, list), 'People saved');
		if (r) peopleOpen = false;
	}

	let nextOpen = $state(false);
	let nextStart = $state('');
	function startNext() {
		if (!m) return;
		const d = new Date(m.starts_at);
		d.setDate(d.getDate() + 7);
		nextStart = localInput(d);
		nextOpen = true;
	}
	async function planNext() {
		const r = await call(() => api.post<Meeting>(`/meetings/${id}/next`, { starts_at: new Date(nextStart).toISOString() }));
		if (r) {
			nextOpen = false;
			toasts.success('Next meeting planned');
			await goto(`/meetings/${(r as Meeting).id}`);
		}
	}

	let minutes = $state('');
	let fileInput = $state<HTMLInputElement>();
	async function attach(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const ids = m?.files.map((f) => f.id) ?? [];
		for (const f of Array.from(input.files ?? [])) {
			const fd = new FormData();
			fd.append('file', f);
			try {
				ids.push((await api.post<{ id: string }>('/files', fd)).id);
			} catch (err) {
				toasts.error(err);
			}
		}
		input.value = '';
		await call(() => api.patch<Meeting>(`/meetings/${id}`, { file_ids: ids }));
	}

	// ---- the agenda ----
	let topic = $state({ title: '', description: '', duration: '', parent_id: '', presenters: [] as string[] });
	async function addTopic(e: SubmitEvent) {
		e.preventDefault();
		const r = await call(() =>
			api.post<Meeting>(`/meetings/${id}/agenda`, {
				title: topic.title,
				description: topic.description || null,
				duration_minutes: topic.duration ? Number(topic.duration) : null,
				parent_id: topic.parent_id || null,
				responsible_ids: topic.presenters
			})
		);
		if (r) topic = { title: '', description: '', duration: '', parent_id: '', presenters: [] };
	}
	async function move(item: AgendaItem, dir: -1 | 1) {
		const ids = topics.map((t) => t.id);
		const i = ids.indexOf(item.id);
		const j = i + dir;
		if (j < 0 || j >= ids.length) return;
		[ids[i], ids[j]] = [ids[j], ids[i]];
		await call(() => api.put<Meeting>(`/meetings/${id}/agenda/order`, { ids }));
	}
	async function removeItem(item: AgendaItem) {
		if (!confirm(`Remove “${item.title}” from the agenda?`)) return;
		await call(() => api.del<Meeting>(`/meetings/agenda/${item.id}`));
	}
	const mayEdit = (item: AgendaItem) => !!m && !closed && (m.can.run || (item.proposed_by?.id === me && m.state === 'prepare'));
	const presents = (item: AgendaItem) => item.responsible.some((p) => p.id === me);
	/** Presenters write their report (shown to them in its editor, not twice). */
	const writes = (item: AgendaItem) => !closed && presents(item) && m?.my_role !== 'viewer';

	let decisions = $state<Record<string, string>>({});
	let reports = $state<Record<string, string>>({});
	const myReport = (item: AgendaItem) => item.reports.find((r) => r.user_id === me)?.body ?? '';

	let taskFor = $state<AgendaItem | null>(null);
	let taskWho = $state('');
	let taskDue = $state('');
	async function makeTask() {
		if (!taskFor) return;
		const r = await call(() => api.post<Meeting>(`/meetings/agenda/${taskFor!.id}/task`, { responsible_id: taskWho || null, deadline: taskDue ? new Date(`${taskDue}T17:00`).toISOString() : null }), 'Task made');
		if (r) taskFor = null;
	}

	let history = $state<{ title: string; items: { meeting: { id: string; title: string; starts_at: string }; decision: string | null; status: string; reports: { name: string; body: string }[] }[] } | null>(null);
	async function showHistory(item: AgendaItem) {
		try {
			history = await api.get(`/meetings/topics/${item.topic_id}/history`);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<svelte:head><title>{m?.title ?? 'Meeting'} · Meetings · NERP</title></svelte:head>

{#snippet agendaItem(item: AgendaItem, n: string, sub: boolean)}
	<article class="space-y-3 {sub ? 'ml-6 border-l-2 border-slate-100 pl-4' : 'card p-4'}" data-topic={item.title}>
		<header class="flex flex-wrap items-start gap-2">
			<span class="font-semibold text-slate-400">{n}</span>
			<div class="min-w-0 flex-1">
				<h3 class="font-medium text-slate-900">{item.title}</h3>
				<p class="text-xs text-slate-500">
					{#if item.responsible.length}Presents: {item.responsible.map((p) => p.name).join(', ')}{:else}No presenter{/if}
					{#if item.duration_minutes} · {item.duration_minutes} min{/if}
					{#if item.proposed_by} · proposed by {item.proposed_by.name}{/if}
				</p>
			</div>
			{#if item.status === 'done'}<span class="rounded-full bg-emerald-50 px-2 py-0.5 text-xs text-emerald-700" data-status="done">Settled</span>{/if}
			{#if item.status === 'carried'}<span class="rounded-full bg-slate-100 px-2 py-0.5 text-xs text-slate-600" data-status="carried">Carried to the next meeting</span>{/if}
			{#if item.earlier}<button class="text-xs text-brand-700 hover:underline" onclick={() => showHistory(item)}>History ({item.earlier})</button>{/if}
			{#if m?.can.run && !closed && !sub}
				<button class="text-slate-400 hover:text-slate-700" aria-label="Move {item.title} up" onclick={() => move(item, -1)}>↑</button>
				<button class="text-slate-400 hover:text-slate-700" aria-label="Move {item.title} down" onclick={() => move(item, 1)}>↓</button>
			{/if}
			{#if mayEdit(item)}<button class="text-slate-400 hover:text-red-600" aria-label="Remove {item.title}" onclick={() => removeItem(item)}><Icon name="trash" class="size-4" /></button>{/if}
		</header>
		{#if item.description}<p class="text-sm whitespace-pre-line text-slate-700">{item.description}</p>{/if}

		{#each item.reports.filter((r) => !(r.user_id === me && writes(item))) as r (r.user_id)}
			<div class="rounded-xl bg-slate-50 px-3 py-2 text-sm" data-report={r.name}>
				<p class="text-xs font-medium text-slate-500">Report from {r.name}</p>
				<p class="whitespace-pre-line text-slate-800">{r.body}</p>
				{#each r.files as f (f.id)}<button class="mt-1 mr-2 text-xs text-brand-700 hover:underline" onclick={() => downloadFile(f.id, f.name)}>📎 {f.name}</button>{/each}
			</div>
		{/each}
		{#if writes(item)}
			<div class="flex gap-2">
				<textarea class="input min-h-16 flex-1 text-sm" aria-label="Your report on {item.title}" placeholder="Your report on this topic…" value={reports[item.id] ?? myReport(item)} oninput={(e) => (reports[item.id] = e.currentTarget.value)}></textarea>
				<button class="btn btn-secondary self-end" onclick={() => call(() => api.put<Meeting>(`/meetings/agenda/${item.id}/report`, { body: reports[item.id] ?? myReport(item) }), 'Report saved')}>Save report</button>
			</div>
		{/if}

		{#if item.decision && !(m?.can.run && running)}<p class="rounded-xl bg-emerald-50/60 px-3 py-2 text-sm text-slate-800" data-decision><span class="font-medium">Decision:</span> {item.decision}</p>{/if}
		{#if m?.can.run && running}
			<div class="space-y-2">
				<textarea class="input min-h-14 text-sm" aria-label="Decision on {item.title}" placeholder="What was decided…" value={decisions[item.id] ?? item.decision ?? ''} oninput={(e) => (decisions[item.id] = e.currentTarget.value)} onblur={() => decisions[item.id] !== undefined && decisions[item.id] !== (item.decision ?? '') && patchItem(item.id, { decision: decisions[item.id] || null })}></textarea>
				<div class="flex flex-wrap items-center gap-3 text-sm">
					<label class="flex items-center gap-2"><input type="checkbox" checked={item.status === 'done'} disabled={item.status === 'carried'} onchange={(e) => patchItem(item.id, { status: e.currentTarget.checked ? 'done' : 'open' })} />Settled</label>
					{#if !item.task}<button class="btn btn-secondary" onclick={() => ((taskFor = item), (taskWho = item.responsible[0]?.id ?? ''), (taskDue = ''))}><Icon name="checkSquare" class="size-4" />Make a task</button>{/if}
				</div>
			</div>
		{/if}
		{#if item.task}<a class="inline-block text-sm text-brand-700 hover:underline" href="/tasks/{item.task.id}" data-task>Task #{item.task.number}: {item.task.title}</a>{/if}
	</article>
{/snippet}

{#if missing}
	<p class="card px-4 py-10 text-center text-slate-500">This meeting doesn't exist or isn't yours to see.</p>
{:else if m}
	<div class="space-y-4">
		<div>
			<a href="/meetings" class="text-sm text-slate-500 hover:underline">‹ Meetings</a>
			<div class="mt-1 flex flex-wrap items-center gap-3">
				<h1 class="text-2xl font-semibold text-slate-900">{m.title}</h1>
				<span class="rounded-full px-2 py-0.5 text-xs {STATE_TONE[m.state]}" data-state={m.state}>{STATE_LABEL[m.state]}</span>
			</div>
			<p class="mt-1 text-sm text-slate-600">
				{meetingTime(m.starts_at, m.duration_minutes)}{m.place ? ` · ${m.place}` : ''}{#if m.project} · <a class="hover:underline" href="/projects/{m.project.id}">{m.project.name}</a>{/if}
			</p>
			<p class="text-sm text-slate-500">
				{#if m.previous}<a class="hover:underline" href="/meetings/{m.previous.id}" data-previous>‹ Previous: {meetingTime(m.previous.starts_at)}</a>{/if}
				{#if m.previous && m.next}<span> · </span>{/if}
				{#if m.next}<a class="hover:underline" href="/meetings/{m.next.id}" data-next>Next: {meetingTime(m.next.starts_at)} ›</a>{/if}
			</p>
			{#if m.description}<p class="mt-2 whitespace-pre-line text-slate-700">{m.description}</p>{/if}
			<div class="mt-3 flex flex-wrap gap-2">
				{#if m.can.run && m.state === 'prepare'}<button class="btn btn-primary" onclick={() => post(`/meetings/${id}/start`, undefined, 'The meeting has started')}>Start the meeting</button>{/if}
				{#if m.can.run && running}<button class="btn btn-primary" onclick={close}>Close and send the minutes</button>{/if}
				{#if m.my_role === 'owner' && closed}<button class="btn btn-secondary" onclick={() => post(`/meetings/${id}/reopen`, undefined, 'Reopened')}>Reopen</button>{/if}
				{#if m.can.run && !m.next}<button class="btn btn-secondary" onclick={startNext}>Plan the next meeting</button>{/if}
				{#if m.can.run && !closed}<button class="btn btn-secondary" onclick={startEdit}>Edit</button>{/if}
				{#if m.event_id && m.my_role !== 'viewer'}<button class="btn btn-secondary" onclick={chat}><Icon name="message" class="size-4" />Meeting chat and call</button>{/if}
				<a class="btn btn-secondary" href="/meetings/{id}/minutes">Minutes</a>
				{#if m.can.delete}<button class="btn btn-secondary text-red-700" onclick={remove}><Icon name="trash" class="size-4" />Delete</button>{/if}
			</div>
		</div>

		<div class="grid gap-4 lg:grid-cols-[1fr_20rem]">
			<section class="min-w-0 space-y-3">
				<h2 class="text-lg font-semibold text-slate-900">Agenda</h2>
				{#each topics as t, i (t.id)}
					{@render agendaItem(t, `${i + 1}.`, false)}
					{#each t.subs as s, j (s.id)}{@render agendaItem(s, `${i + 1}.${j + 1}`, true)}{/each}
				{:else}
					<p class="card px-4 py-8 text-center text-slate-500">No topics yet.</p>
				{/each}

				{#if m.can.propose}
					<form class="card space-y-3 p-4" onsubmit={addTopic}>
						<h3 class="font-medium text-slate-900">{m.can.run ? 'Add a topic' : 'Propose a topic'}</h3>
						<div class="grid gap-3 sm:grid-cols-[1fr_7rem]">
							<input class="input" aria-label="Topic" placeholder="Topic" required maxlength="300" bind:value={topic.title} />
							<input class="input" aria-label="Minutes for the topic" type="number" min="1" max="600" placeholder="min" bind:value={topic.duration} />
						</div>
						<textarea class="input min-h-14" aria-label="Topic description" placeholder="Details (optional)" bind:value={topic.description}></textarea>
						{#if topics.length}
							<select class="input" aria-label="Under topic" bind:value={topic.parent_id}>
								<option value="">A topic of its own</option>
								{#each topics as t (t.id)}<option value={t.id}>A sub-point of “{t.title}”</option>{/each}
							</select>
						{/if}
						<fieldset class="flex flex-wrap gap-3 text-sm">
							<legend class="label">Presented by</legend>
							{#each m.members as p (p.id)}
								<label class="flex items-center gap-1.5"><input type="checkbox" value={p.id} bind:group={topic.presenters} />{p.name}</label>
							{/each}
						</fieldset>
						<button class="btn btn-primary" type="submit">{m.can.run ? 'Add' : 'Propose'}</button>
					</form>
				{/if}
			</section>

			<aside class="space-y-4">
				<section class="card p-4">
					<div class="mb-2 flex items-center justify-between">
						<h2 class="font-medium text-slate-900">People</h2>
						{#if m.can.manage_people && !closed}<button class="text-sm text-brand-700 hover:underline" onclick={startPeople}>Change</button>{/if}
					</div>
					<ul class="space-y-2 text-sm">
						{#each m.members as p (p.id)}
							<li class="flex flex-wrap items-center gap-2" data-person={p.name}>
								<span class="flex-1"><span class="text-slate-900">{p.name}</span> <span class="text-xs text-slate-500">{ROLE_LABEL[p.role]}</span></span>
								{#if m.can.run && running}
									<button class="rounded px-1.5 py-0.5 text-xs {p.attended === true ? 'bg-emerald-100 text-emerald-800' : 'text-slate-500 hover:bg-slate-100'}" onclick={() => post(`/meetings/${id}/attendance`, { user_id: p.id, attended: p.attended === true ? null : true })}>Present</button>
									<button class="rounded px-1.5 py-0.5 text-xs {p.attended === false ? 'bg-red-50 text-red-700' : 'text-slate-500 hover:bg-slate-100'}" onclick={() => post(`/meetings/${id}/attendance`, { user_id: p.id, attended: p.attended === false ? null : false })}>Absent</button>
								{:else if p.attended !== null}
									<span class="text-xs {p.attended ? 'text-emerald-700' : 'text-red-600'}">{p.attended ? 'present' : 'absent'}</span>
								{/if}
							</li>
						{/each}
					</ul>
				</section>

				<section class="card space-y-2 p-4">
					<h2 class="font-medium text-slate-900">Files</h2>
					{#each m.files as f (f.id)}<button class="block text-left text-sm text-brand-700 hover:underline" onclick={() => downloadFile(f.id, f.name)}>📎 {f.name}</button>{:else}<p class="text-sm text-slate-500">None.</p>{/each}
					{#if m.can.run && !closed}
						<button class="btn btn-secondary" onclick={() => fileInput?.click()}><Icon name="paperclip" class="size-4" />Attach</button>
						<input bind:this={fileInput} class="hidden" type="file" multiple aria-label="Attach files" onchange={attach} />
					{/if}
				</section>

				<section class="card space-y-2 p-4">
					<h2 class="font-medium text-slate-900">Notes for the minutes</h2>
					{#if m.can.run}
						<textarea class="input min-h-28 text-sm" aria-label="Notes for the minutes" bind:value={minutes}></textarea>
						<button class="btn btn-secondary" disabled={minutes === (m.minutes ?? '')} onclick={() => call(() => api.patch<Meeting>(`/meetings/${id}`, { minutes: minutes || null }), 'Saved')}>Save notes</button>
					{:else}
						<p class="text-sm whitespace-pre-line text-slate-700">{m.minutes || 'None yet.'}</p>
					{/if}
				</section>
			</aside>
		</div>
	</div>

	<Modal bind:open={editOpen} title="Edit the meeting">
		<form id="edit-meeting" class="space-y-3" onsubmit={saveEdit}>
			<div><label class="label" for="e-title">Title</label><input id="e-title" class="input" required bind:value={ed.title} /></div>
			<div class="grid gap-3 sm:grid-cols-[1fr_7rem]">
				<div><label class="label" for="e-start">Starts</label><input id="e-start" class="input" type="datetime-local" required bind:value={ed.start} /></div>
				<div><label class="label" for="e-dur">Minutes</label><input id="e-dur" class="input" type="number" min="5" max="1440" bind:value={ed.duration} /></div>
			</div>
			<div><label class="label" for="e-place">Place</label><input id="e-place" class="input" bind:value={ed.place} /></div>
			<div><label class="label" for="e-desc">What it is about</label><textarea id="e-desc" class="input min-h-20" bind:value={ed.description}></textarea></div>
		</form>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (editOpen = false)}>Cancel</button>
			<button class="btn btn-primary" form="edit-meeting" type="submit">Save</button>
		{/snippet}
	</Modal>

	<Modal bind:open={peopleOpen} title="People">
		<div class="space-y-3">
			<MultiUserPicker bind:value={picked} exclude={[m.members.find((x) => x.role === 'owner')?.id ?? '']} />
			<div>
				<label class="label" for="p-keeper">Writes the minutes</label>
				<select id="p-keeper" class="input" bind:value={keeperId}><option value="">Nobody</option>{#each picked as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select>
			</div>
		</div>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (peopleOpen = false)}>Cancel</button>
			<button class="btn btn-primary" onclick={savePeople}>Save</button>
		{/snippet}
	</Modal>

	<Modal bind:open={nextOpen} title="Plan the next meeting">
		<p class="mb-3 text-sm text-slate-600">The same people, place and workgroup. Every topic not settled moves to its agenda.</p>
		<label class="label" for="n-start">Starts</label><input id="n-start" class="input" type="datetime-local" bind:value={nextStart} />
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (nextOpen = false)}>Cancel</button>
			<button class="btn btn-primary" onclick={planNext}>Plan it</button>
		{/snippet}
	</Modal>

	<Modal open={!!taskFor} title="Make a task" onclose={() => (taskFor = null)}>
		{#if taskFor}
			<p class="mb-3 text-sm text-slate-600">“{taskFor.title}”, with the decision and a link to this meeting.</p>
			<div class="space-y-3">
				<div>
					<label class="label" for="t-who">Responsible</label>
					<select id="t-who" class="input" bind:value={taskWho}><option value="">Me</option>{#each m.members as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select>
				</div>
				<div><label class="label" for="t-due">Deadline</label><input id="t-due" class="input" type="date" bind:value={taskDue} /></div>
			</div>
		{/if}
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (taskFor = null)}>Cancel</button>
			<button class="btn btn-primary" onclick={makeTask}>Make the task</button>
		{/snippet}
	</Modal>

	<Modal open={!!history} title={history ? `History: ${history.title}` : ''} onclose={() => (history = null)} size="lg">
		{#if history}
			<ol class="space-y-3">
				{#each history.items as h (h.meeting.id)}
					<li class="rounded-xl border border-slate-200 p-3 text-sm" data-history={h.meeting.title}>
						<a class="font-medium text-slate-900 hover:underline" href="/meetings/{h.meeting.id}" onclick={() => (history = null)}>{h.meeting.title}, {meetingTime(h.meeting.starts_at)}</a>
						{#each h.reports as r (r.name)}<p class="mt-1 text-slate-600"><span class="font-medium">{r.name}:</span> {r.body}</p>{/each}
						<p class="mt-1 text-slate-800">{h.decision ? `Decision: ${h.decision}` : 'No decision recorded.'} <span class="text-xs text-slate-500">({h.status})</span></p>
					</li>
				{/each}
			</ol>
		{/if}
	</Modal>
{/if}
