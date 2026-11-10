<script lang="ts">
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import type { ProjectMember, ScrumOverview } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Modal from '../Modal.svelte';

	let {
		open = $bindable(false),
		projectId,
		overview,
		members,
		onchanged
	}: {
		open?: boolean;
		projectId: string;
		overview: ScrumOverview;
		members: ProjectMember[];
		onchanged: () => void;
	} = $props();

	let form = $state({ master: '', days: 14, dod: '' });
	// Filled each time the dialog opens (not when the team changes underneath it).
	$effect(() => {
		if (!open) return;
		untrack(() => {
			meetOpen = false;
			form = { master: overview.scrum_master?.id ?? '', days: overview.sprint_days, dod: overview.dod.map((d) => d.title).join('\n') };
		});
	});

	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			await api.patch(`/projects/${projectId}/scrum`, { scrum_master_id: form.master || null, sprint_days: Number(form.days) });
			await api.put(`/projects/${projectId}/dod`, { items: form.dod.split('\n') });
			open = false;
			onchanged();
		} catch (err) {
			toasts.error(err);
		}
	}

	// ----- team meetings on the group's calendar -----
	const MEETINGS = {
		daily: { title: 'Daily stand-up', minutes: 15, time: '10:00', repeat: { freq: 'weekly', weekdays: [1, 2, 3, 4, 5] } },
		planning: { title: 'Sprint planning', minutes: 120, time: '10:30', repeat: null },
		review: { title: 'Sprint review', minutes: 60, time: '14:00', repeat: null },
		retro: { title: 'Retrospective', minutes: 60, time: '15:30', repeat: null }
	} as const;
	type Kind = keyof typeof MEETINGS;
	let meetOpen = $state(false);
	let meet = $state({ kind: 'daily' as Kind, date: '', time: '10:00', minutes: 15 });
	function openMeetings() {
		meet = { kind: 'daily', date: new Date(Date.now() + 86400e3).toISOString().slice(0, 10), time: MEETINGS.daily.time, minutes: MEETINGS.daily.minutes };
		meetOpen = true;
	}
	function pick(kind: Kind) {
		meet = { ...meet, kind, time: MEETINGS[kind].time, minutes: MEETINGS[kind].minutes };
	}
	async function plan() {
		const m = MEETINGS[meet.kind];
		const start = new Date(`${meet.date}T${meet.time}:00`);
		try {
			await api.post('/calendar/events', {
				title: m.title,
				start: start.toISOString(),
				end: new Date(+start + meet.minutes * 60e3).toISOString(),
				timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
				repeat: m.repeat,
				project_id: projectId,
				remind_minutes: 10,
				attendees: members.filter((x) => x.role !== 'viewer').map((x) => x.user_id)
			});
			toasts.success(`${m.title} is on the team's calendar`);
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<Modal bind:open title="Scrum settings">
	<form id="scrum-settings" class="space-y-4" onsubmit={save}>
		<div class="grid gap-3 sm:grid-cols-2">
			<div>
				<label class="label" for="sc-master">Scrum master</label>
				<select id="sc-master" class="input" bind:value={form.master}>
					<option value="">Nobody</option>
					{#each members.filter((m) => m.role !== 'viewer') as m (m.user_id)}<option value={m.user_id}>{m.full_name}</option>{/each}
				</select>
			</div>
			<div><label class="label" for="sc-days">Sprint length (days)</label><input id="sc-days" type="number" min="1" max="90" class="input" required bind:value={form.days} /></div>
		</div>
		<div>
			<label class="label" for="sc-dod">Definition of done <span class="font-normal text-slate-500">(one point per line; checked before a task is finished)</span></label>
			<textarea id="sc-dod" class="input" rows="5" placeholder={'Code reviewed\nTests pass\nDocumentation updated'} bind:value={form.dod}></textarea>
		</div>
		<div class="border-t border-slate-100 pt-4">
			{#if !meetOpen}
				<button type="button" class="btn btn-secondary" onclick={openMeetings}>Plan team meetings…</button>
			{:else}
				<div class="space-y-3 rounded-lg border border-slate-200 p-3" aria-label="Team meeting">
					<div class="flex flex-wrap gap-2" role="radiogroup" aria-label="Meeting">
						{#each Object.entries(MEETINGS) as [k, m] (k)}
							<button type="button" role="radio" aria-checked={meet.kind === k} class="btn py-1 text-xs {meet.kind === k ? 'btn-primary' : 'btn-secondary'}" onclick={() => pick(k as Kind)}>{m.title}</button>
						{/each}
					</div>
					<div class="grid gap-3 sm:grid-cols-3">
						<div><label class="label" for="sm-date">{meet.kind === 'daily' ? 'From' : 'On'}</label><input id="sm-date" type="date" class="input" bind:value={meet.date} /></div>
						<div><label class="label" for="sm-time">At</label><input id="sm-time" type="time" class="input" bind:value={meet.time} /></div>
						<div><label class="label" for="sm-min">Minutes</label><input id="sm-min" type="number" min="5" max="480" class="input" bind:value={meet.minutes} /></div>
					</div>
					<p class="text-xs text-slate-500">{meet.kind === 'daily' ? 'Every weekday, on' : 'On'} the team's calendar, with every member invited and reminded 10 minutes before.</p>
					<div class="flex gap-2">
						<button type="button" class="btn btn-primary" disabled={!meet.date || !meet.time} onclick={plan}>Add to calendar</button>
						<button type="button" class="btn btn-ghost" onclick={() => (meetOpen = false)}>Close</button>
					</div>
				</div>
			{/if}
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="scrum-settings">Save</button>
	{/snippet}
</Modal>
