<script lang="ts">
	// Help-desk settings ("Manage the help desk"): the inbox and public page, closing and
	// reopening, the lists people choose from, SLAs, and frequently used answers.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { duration, fromHhmm, hhmm, KIND_LABEL, loadDictionary, nameOf, WEEKDAYS, type DictKind, type Entry, type Sla } from '#lib/support.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Settings = { mailbox_id: string | null; mailbox_email: string | null; portal_enabled: boolean; portal_intro: string | null; portal_url: string; auto_close_days: number; allow_reopen: boolean; timezone: string };
	let tab = $state<'general' | 'lists' | 'slas' | 'answers'>('general');
	let settings = $state<Settings | null>(null);
	let mailboxes = $state<{ id: string; email: string }[]>([]);
	let dict = $state<Entry[]>([]);
	let slas = $state<Sla[]>([]);
	let answers = $state<{ id: string; title: string; body: string; sort: number }[]>([]);
	let team = $state<{ id: string; name: string }[]>([]);
	const timezones = ['UTC', ...(Intl.supportedValuesOf?.('timeZone') ?? []).filter((z) => z !== 'UTC')];

	async function loadAll() {
		try {
			[settings, mailboxes, dict, slas, answers, team] = await Promise.all([
				api.get<Settings>('/support/settings'),
				api.get<{ items: { id: string; email: string }[] }>('/crm/mailboxes').then((r) => r.items),
				loadDictionary(),
				api.get<{ items: Sla[] }>('/support/slas').then((r) => r.items),
				api.get<{ items: { id: string; title: string; body: string; sort: number }[] }>('/support/answers').then((r) => r.items),
				api.get<{ items: { id: string; name: string }[] }>('/support/team').then((r) => r.items)
			]);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(loadAll);

	async function saveSettings(e: SubmitEvent) {
		e.preventDefault();
		if (!settings) return;
		try {
			settings = await api.put<Settings>('/support/settings', {
				mailbox_id: settings.mailbox_id || null,
				portal_enabled: settings.portal_enabled,
				portal_intro: settings.portal_intro || null,
				auto_close_days: Number(settings.auto_close_days),
				allow_reopen: settings.allow_reopen,
				timezone: settings.timezone
			});
			toasts.success('Saved');
		} catch (err) {
			toasts.error(err);
		}
	}

	// ---- lists ----
	let adding = $state<Record<string, string>>({});
	async function addEntry(kind: DictKind) {
		const name = (adding[kind] ?? '').trim();
		if (!name) return;
		try {
			await api.post('/support/dictionary', { kind, name });
			adding[kind] = '';
			dict = await loadDictionary();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function updateEntry(id: string, body: Record<string, unknown>) {
		try {
			await api.patch(`/support/dictionary/${id}`, body);
			dict = await loadDictionary();
		} catch (e) {
			toasts.error(e);
			dict = await loadDictionary();
		}
	}
	async function removeEntry(en: Entry) {
		if (!confirm(`Remove “${en.name}”? Tickets that have it keep working without it.`)) return;
		try {
			await api.del(`/support/dictionary/${en.id}`);
			dict = await loadDictionary();
		} catch (e) {
			toasts.error(e);
		}
	}

	// ---- SLAs ----
	type Draft = Omit<Sla, 'id' | 'is_default' | 'hours'> & { id: string | null; is_default: boolean; days: { on: boolean; from: string; till: string; from2: string; till2: string }[] };
	let editing = $state<Draft | null>(null);
	let slaOpen = $state(false);
	function edit(s: Sla | null) {
		const h = s?.hours ?? Object.fromEntries([1, 2, 3, 4, 5].map((d) => [String(d), [[540, 1080]]]));
		editing = {
			id: s?.id ?? null,
			name: s?.name ?? '',
			description: s?.description ?? '',
			priority: s?.priority ?? 0,
			is_default: s?.is_default ?? false,
			response_minutes: s?.response_minutes ?? 480,
			notice_minutes: s?.notice_minutes ?? 60,
			responsible_id: s?.responsible_id ?? null,
			category_ids: [...(s?.category_ids ?? [])],
			criticality_ids: [...(s?.criticality_ids ?? [])],
			requesters: s?.requesters ?? 'any',
			skip_holidays: s?.skip_holidays ?? true,
			days: [1, 2, 3, 4, 5, 6, 7].map((d) => {
				const spans = h[String(d)] ?? [];
				const end = (m: number) => (m >= 1440 ? '00:00' : hhmm(m));
				return { on: spans.length > 0, from: hhmm(spans[0]?.[0] ?? 540), till: end(spans[0]?.[1] ?? 1080), from2: spans[1] ? hhmm(spans[1][0]) : '', till2: spans[1] ? end(spans[1][1]) : '' };
			})
		};
		slaOpen = true;
	}
	async function saveSla(e: SubmitEvent) {
		e.preventDefault();
		if (!editing) return;
		const hours: Record<string, [number, number][]> = {};
		editing.days.forEach((d, i) => {
			if (!d.on) return;
			const end = (s: string) => (s === '00:00' || s === '24:00' ? 1440 : fromHhmm(s));
			const spans: [number, number][] = [[fromHhmm(d.from), end(d.till)]];
			if (d.from2 && d.till2) spans.push([fromHhmm(d.from2), end(d.till2)]);
			hours[String(i + 1)] = spans;
		});
		const body = {
			name: editing.name,
			description: editing.description || null,
			priority: Number(editing.priority),
			response_minutes: Number(editing.response_minutes),
			notice_minutes: Number(editing.notice_minutes),
			responsible_id: editing.responsible_id || null,
			category_ids: editing.category_ids,
			criticality_ids: editing.criticality_ids,
			requesters: editing.requesters,
			skip_holidays: editing.skip_holidays,
			hours
		};
		try {
			if (editing.id) await api.patch(`/support/slas/${editing.id}`, body);
			else await api.post('/support/slas', body);
			slaOpen = false;
			slas = (await api.get<{ items: Sla[] }>('/support/slas')).items;
			toasts.success('SLA saved');
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeSla(s: Sla) {
		if (!confirm(`Remove the SLA “${s.name}”? Its tickets move to the default one.`)) return;
		try {
			await api.del(`/support/slas/${s.id}`);
			slas = slas.filter((x) => x.id !== s.id);
		} catch (e) {
			toasts.error(e);
		}
	}
	const toggle = (list: string[], id: string) => (list.includes(id) ? list.filter((x) => x !== id) : [...list, id]);

	// ---- answers ----
	let answer = $state<{ id: string | null; title: string; body: string } | null>(null);
	let answerOpen = $state(false);
	async function saveAnswer(e: SubmitEvent) {
		e.preventDefault();
		if (!answer) return;
		try {
			if (answer.id) await api.patch(`/support/answers/${answer.id}`, { title: answer.title, body: answer.body });
			else await api.post('/support/answers', { title: answer.title, body: answer.body });
			answerOpen = false;
			answers = (await api.get<{ items: typeof answers }>('/support/answers')).items;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeAnswer(id: string) {
		if (!confirm('Remove this answer?')) return;
		await api.del(`/support/answers/${id}`).catch(toasts.error);
		answers = answers.filter((a) => a.id !== id);
	}
</script>

<svelte:head><title>Settings · Help desk · NERP</title></svelte:head>

<div class="max-w-5xl space-y-4">
	<div><a href="/support" class="text-sm text-slate-500 hover:underline">‹ Help desk</a><h1 class="mt-1 text-2xl font-semibold text-slate-900">Help-desk settings</h1></div>
	<div class="flex flex-wrap gap-1 border-b border-slate-200" role="tablist">
		{#each [['general', 'General'], ['lists', 'Lists'], ['slas', 'SLAs'], ['answers', 'Frequently used answers']] as [k, label] (k)}
			<button role="tab" aria-selected={tab === k} class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {tab === k ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500 hover:text-slate-800'}" onclick={() => (tab = k as typeof tab)}>{label}</button>
		{/each}
	</div>

	{#if tab === 'general' && settings}
		<form class="card max-w-2xl space-y-4 p-5" onsubmit={saveSettings}>
			<div>
				<label class="label" for="s-mb">Help-desk inbox</label>
				<select id="s-mb" class="input" bind:value={settings.mailbox_id}>
					<option value={null}>None: customers answer on their request page</option>
					{#each mailboxes as m (m.id)}<option value={m.id}>{m.email}</option>{/each}
				</select>
				<p class="mt-1 text-xs text-slate-500">Mail to this mailbox becomes tickets (it no longer goes to the CRM), and answers go out through it. Connect mailboxes under CRM → Mailboxes.</p>
			</div>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={settings.portal_enabled} />Public help page at <a class="text-brand-700 hover:underline" href={settings.portal_url} target="_blank" rel="noopener">{settings.portal_url}</a></label>
			<div><label class="label" for="s-intro">Text on the help page</label><textarea id="s-intro" class="input min-h-20" bind:value={settings.portal_intro}></textarea></div>
			<div class="grid gap-3 sm:grid-cols-2">
				<div><label class="label" for="s-days">Close tickets waiting for the customer after (days; 0: never)</label><input id="s-days" class="input" type="number" min="0" max="365" bind:value={settings.auto_close_days} /></div>
				<div>
					<label class="label" for="s-tz">Time zone of working hours</label>
					<select id="s-tz" class="input" bind:value={settings.timezone}>{#each timezones as z (z)}<option value={z}>{z}</option>{/each}</select>
				</div>
			</div>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={settings.allow_reopen} />Customers reopen a closed request by writing to it</label>
			<button class="btn btn-primary" type="submit">Save</button>
		</form>
	{:else if tab === 'lists'}
		<div class="grid gap-4 md:grid-cols-2">
			{#each Object.entries(KIND_LABEL) as [kind, label] (kind)}
				<section class="card p-4" data-kind={kind}>
					<h2 class="mb-2 font-medium text-slate-900">{label}</h2>
					<ul class="divide-y divide-slate-100">
						{#each dict.filter((e) => e.kind === kind) as en (en.id)}
							<li class="flex flex-wrap items-center gap-2 py-2 text-sm" data-entry={en.name}>
								<input class="input flex-1" aria-label="Name" value={en.name} onchange={(e) => updateEntry(en.id, { name: e.currentTarget.value })} />
								{#if kind === 'category'}
									<select class="input w-36" aria-label="Goes to" value={en.responsible_id ?? ''} onchange={(e) => updateEntry(en.id, { responsible_id: e.currentTarget.value || null })}>
										<option value="">Nobody</option>
										{#each team as u (u.id)}<option value={u.id}>{u.name}</option>{/each}
									</select>
								{/if}
								{#if !['mark', 'difficulty'].includes(kind)}
									<label class="flex items-center gap-1 text-xs text-slate-500"><input type="radio" name="default-{kind}" checked={en.is_default} onchange={() => updateEntry(en.id, { is_default: true })} />default</label>
								{/if}
								<button class="text-slate-400 hover:text-red-600" aria-label="Remove {en.name}" onclick={() => removeEntry(en)}><Icon name="trash" class="size-4" /></button>
							</li>
						{/each}
					</ul>
					<form class="mt-2 flex gap-2" onsubmit={(e) => (e.preventDefault(), addEntry(kind as DictKind))}>
						<input class="input flex-1" aria-label="New {label.toLowerCase()} entry" placeholder="Add…" bind:value={adding[kind]} />
						<button class="btn btn-secondary" type="submit">Add</button>
					</form>
				</section>
			{/each}
		</div>
	{:else if tab === 'slas'}
		<div class="space-y-3">
			<p class="text-sm text-slate-600">A ticket gets the matching SLA with the highest priority, else the default one. Times count working hours only.</p>
			<button class="btn btn-primary" onclick={() => edit(null)}><Icon name="plus" class="size-4" />New SLA</button>
			{#each slas as s (s.id)}
				<section class="card flex flex-wrap items-center gap-3 p-4" data-sla={s.name}>
					<div class="min-w-0 flex-1">
						<h2 class="font-medium text-slate-900">{s.name}{#if s.is_default}<span class="ml-2 rounded bg-slate-100 px-1.5 text-xs text-slate-600">default</span>{/if}</h2>
						<p class="text-sm text-slate-500">
							Reply within {duration(s.response_minutes)}{s.notice_minutes ? `, warn ${duration(s.notice_minutes)} before` : ''} ·
							{s.category_ids.length ? s.category_ids.map((c) => nameOf(dict, c)).join(', ') : 'any category'} ·
							{s.criticality_ids.length ? s.criticality_ids.map((c) => nameOf(dict, c)).join(', ') : 'any criticality'} · priority {s.priority}
						</p>
					</div>
					<button class="btn btn-secondary" onclick={() => edit(s)}>Edit</button>
					{#if !s.is_default}<button class="btn btn-secondary" aria-label="Remove {s.name}" onclick={() => removeSla(s)}><Icon name="trash" class="size-4" /></button>{/if}
				</section>
			{/each}
		</div>
	{:else if tab === 'answers'}
		<div class="space-y-3">
			<button class="btn btn-primary" onclick={() => ((answer = { id: null, title: '', body: '' }), (answerOpen = true))}><Icon name="plus" class="size-4" />New answer</button>
			{#each answers as a (a.id)}
				<section class="card p-4" data-answer={a.title}>
					<div class="flex items-center gap-2">
						<h2 class="flex-1 font-medium text-slate-900">{a.title}</h2>
						<button class="btn btn-secondary" onclick={() => ((answer = { id: a.id, title: a.title, body: a.body }), (answerOpen = true))}>Edit</button>
						<button class="btn btn-secondary" aria-label="Remove {a.title}" onclick={() => removeAnswer(a.id)}><Icon name="trash" class="size-4" /></button>
					</div>
					<p class="mt-1 text-sm whitespace-pre-line text-slate-600">{a.body}</p>
				</section>
			{:else}<p class="text-sm text-slate-500">No answers yet.</p>{/each}
		</div>
	{/if}
</div>

<Modal bind:open={slaOpen} title={editing?.id ? 'Edit SLA' : 'New SLA'} size="lg">
	{#if editing}
		<form id="sla-form" class="space-y-4" onsubmit={saveSla}>
			<div class="grid gap-3 sm:grid-cols-[1fr_8rem]">
				<div><label class="label" for="sla-name">Name</label><input id="sla-name" class="input" required bind:value={editing.name} /></div>
				<div><label class="label" for="sla-prio">Priority</label><input id="sla-prio" class="input" type="number" bind:value={editing.priority} /></div>
			</div>
			<div class="grid gap-3 sm:grid-cols-2">
				<div><label class="label" for="sla-resp">Reply within (minutes of working time)</label><input id="sla-resp" class="input" type="number" min="1" required bind:value={editing.response_minutes} /><p class="mt-1 text-xs text-slate-500">{duration(editing.response_minutes)}</p></div>
				<div><label class="label" for="sla-notice">Warn this many minutes before (0: no)</label><input id="sla-notice" class="input" type="number" min="0" bind:value={editing.notice_minutes} /></div>
			</div>
			<div class="grid gap-3 sm:grid-cols-2">
				<div>
					<label class="label" for="sla-who">Responsible (told when overdue)</label>
					<select id="sla-who" class="input" bind:value={editing.responsible_id}><option value={null}>Nobody</option>{#each team as u (u.id)}<option value={u.id}>{u.name}</option>{/each}</select>
				</div>
				<div>
					<label class="label" for="sla-req">For requests from</label>
					<select id="sla-req" class="input" bind:value={editing.requesters}><option value="any">Anyone</option><option value="staff">Employees</option><option value="external">Customers</option></select>
				</div>
			</div>
			{#if !editing.is_default}
				{#each [['category', 'category_ids', 'Categories'], ['criticality', 'criticality_ids', 'Criticality']] as [kind, field, label] (kind)}
					<fieldset>
						<legend class="label">{label} (none ticked: any)</legend>
						<div class="flex flex-wrap gap-3 text-sm">
							{#each dict.filter((e) => e.kind === kind) as en (en.id)}
								<label class="flex items-center gap-1.5"><input type="checkbox" checked={(editing as unknown as Record<string, string[]>)[field].includes(en.id)} onchange={() => editing && ((editing as unknown as Record<string, string[]>)[field] = toggle((editing as unknown as Record<string, string[]>)[field], en.id))} />{en.name}</label>
							{/each}
						</div>
					</fieldset>
				{/each}
			{/if}
			<fieldset>
				<legend class="label">Working hours</legend>
				<div class="space-y-1.5 text-sm">
					{#each editing.days as d, i (i)}
						<div class="flex flex-wrap items-center gap-2" data-day={i + 1}>
							<label class="flex w-32 items-center gap-2"><input type="checkbox" bind:checked={d.on} />{WEEKDAYS[i]}</label>
							{#if d.on}
								<input class="input w-28" type="time" aria-label="{WEEKDAYS[i]} from" bind:value={d.from} /> – <input class="input w-28" type="time" aria-label="{WEEKDAYS[i]} till" bind:value={d.till} />
								<span class="text-slate-400">and</span>
								<input class="input w-28" type="time" aria-label="{WEEKDAYS[i]} from (second)" bind:value={d.from2} /> – <input class="input w-28" type="time" aria-label="{WEEKDAYS[i]} till (second)" bind:value={d.till2} />
							{/if}
						</div>
					{/each}
				</div>
				<p class="mt-1 text-xs text-slate-500">Till 00:00 means the end of the day.</p>
			</fieldset>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" bind:checked={editing.skip_holidays} />Company holidays are not worked</label>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (slaOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="sla-form" type="submit">Save</button>
	{/snippet}
</Modal>

<Modal bind:open={answerOpen} title={answer?.id ? 'Edit answer' : 'New answer'}>
	{#if answer}
		<form id="answer-form" class="space-y-3" onsubmit={saveAnswer}>
			<div><label class="label" for="a-title">Title</label><input id="a-title" class="input" required bind:value={answer.title} /></div>
			<div><label class="label" for="a-body">Text</label><textarea id="a-body" class="input min-h-32" required bind:value={answer.body}></textarea></div>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (answerOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="answer-form" type="submit">Save</button>
	{/snippet}
</Modal>
