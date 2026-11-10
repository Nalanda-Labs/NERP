<script lang="ts">
	// Meetings: the ones coming up and the past ones, and planning a new one.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { meetingTime, STATE_LABEL, STATE_TONE, type Meeting, type MeetingSummary } from '#lib/meetings.ts';
	import type { ProjectSummary } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let scope = $state<'upcoming' | 'past'>('upcoming');
	let q = $state('');
	let items = $state<MeetingSummary[]>([]);
	let loaded = $state(false);
	let groups = $state<ProjectSummary[]>([]);

	async function load() {
		try {
			items = (await api.get<{ items: MeetingSummary[] }>(`/meetings${query({ scope, q })}`)).items;
		} catch (e) {
			toasts.error(e);
		} finally {
			loaded = true;
		}
	}
	onMount(async () => {
		await load();
		groups = (await api.get<ProjectSummary[]>('/projects?mine=true').catch(() => [])).filter((g) => g.my_role && g.my_role !== 'viewer');
	});
	let timer: ReturnType<typeof setTimeout>;
	const search = () => {
		clearTimeout(timer);
		timer = setTimeout(load, 250);
	};

	// ---- planning one ----
	const tz = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
	const pad = (n: number) => String(n).padStart(2, '0');
	const localInput = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
	let open = $state(false);
	let saving = $state(false);
	let f = $state({ title: '', start: '', duration: 60, place: '', description: '', project_id: '' });
	let people = $state<PickedUser[]>([]);
	let keeper = $state<string | null>(null);
	function start() {
		const d = new Date();
		d.setDate(d.getDate() + 1);
		d.setHours(10, 0, 0, 0);
		f = { title: '', start: localInput(d), duration: 60, place: '', description: '', project_id: '' };
		people = [];
		keeper = null;
		open = true;
	}
	async function create(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			const members = people.map((p) => ({ user_id: p.id, role: p.id === keeper ? 'keeper' : 'member' }));
			if (keeper && !people.some((p) => p.id === keeper)) members.push({ user_id: keeper, role: 'keeper' });
			const m = await api.post<Meeting>('/meetings', {
				title: f.title,
				starts_at: new Date(f.start).toISOString(),
				duration_minutes: Number(f.duration),
				timezone: tz,
				place: f.place || null,
				description: f.description || null,
				project_id: f.project_id || null,
				members
			});
			open = false;
			await goto(`/meetings/${m.id}`);
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
</script>

<svelte:head><title>Meetings · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Meetings</h1>
		<button class="btn btn-primary" onclick={start}><Icon name="plus" class="size-4" />New meeting</button>
	</div>
	<div class="flex flex-wrap items-center gap-3">
		<div class="flex gap-1 border-b border-slate-200" role="tablist">
			{#each [['upcoming', 'Coming up'], ['past', 'Past']] as [k, label] (k)}
				<button role="tab" aria-selected={scope === k} class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {scope === k ? 'border-brand-600 text-brand-700' : 'border-transparent text-slate-500 hover:text-slate-800'}" onclick={() => ((scope = k as typeof scope), load())}>{label}</button>
			{/each}
		</div>
		<input class="input max-w-xs" type="search" placeholder="Search meetings" aria-label="Search meetings" bind:value={q} oninput={search} />
	</div>

	<div class="space-y-2">
		{#each items as m (m.id)}
			<a href="/meetings/{m.id}" class="card flex flex-wrap items-center gap-x-4 gap-y-1 p-4 hover:border-brand-200" data-meeting={m.title}>
				<div class="w-44 shrink-0 text-sm text-slate-600">{meetingTime(m.starts_at, m.duration_minutes)}</div>
				<div class="min-w-0 flex-1">
					<p class="font-medium text-slate-900">{m.title}</p>
					<p class="text-xs text-slate-500">{m.people} people · {m.topics} topics{m.place ? ` · ${m.place}` : ''}{m.project ? ` · ${m.project.name}` : ''}</p>
				</div>
				<span class="rounded-full px-2 py-0.5 text-xs {STATE_TONE[m.state]}">{STATE_LABEL[m.state]}</span>
			</a>
		{:else}
			{#if loaded}<p class="card px-4 py-10 text-center text-slate-500">{scope === 'upcoming' ? 'No meetings coming up.' : 'No past meetings.'}</p>{/if}
		{/each}
	</div>
</div>

<Modal bind:open title="New meeting" size="lg">
	<form id="new-meeting" class="space-y-4" onsubmit={create}>
		<div><label class="label" for="m-title">Title</label><input id="m-title" class="input" required maxlength="300" bind:value={f.title} /></div>
		<div class="grid gap-3 sm:grid-cols-[1fr_8rem]">
			<div><label class="label" for="m-start">Starts</label><input id="m-start" class="input" type="datetime-local" required bind:value={f.start} /></div>
			<div><label class="label" for="m-dur">Minutes</label><input id="m-dur" class="input" type="number" min="5" max="1440" step="5" bind:value={f.duration} /></div>
		</div>
		<div class="grid gap-3 sm:grid-cols-2">
			<div><label class="label" for="m-place">Place</label><input id="m-place" class="input" placeholder="Room, or online" bind:value={f.place} /></div>
			<div>
				<label class="label" for="m-group">Workgroup</label>
				<select id="m-group" class="input" bind:value={f.project_id}><option value="">None</option>{#each groups as g (g.id)}<option value={g.id}>{g.name}</option>{/each}</select>
			</div>
		</div>
		<div><span class="label">People</span><MultiUserPicker bind:value={people} /></div>
		<div><span class="label">Writes the minutes (optional)</span><UserPicker bind:value={keeper} placeholder="Choose the keeper…" /></div>
		<div><label class="label" for="m-desc">What it is about</label><textarea id="m-desc" class="input min-h-20" bind:value={f.description}></textarea></div>
		<p class="text-xs text-slate-500">It goes on everyone's calendar ({tz}).</p>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="new-meeting" type="submit" disabled={saving}>Plan it</button>
	{/snippet}
</Modal>
