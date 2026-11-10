<script lang="ts">
	// A project board's automation: rules per column, triggers that move tasks, and the log.
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { STATUS, type Stage } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from './Icon.svelte';
	import Modal from './Modal.svelte';
	import UserPicker from './UserPicker.svelte';

	let { open = $bindable(false), projectId, stages }: { open?: boolean; projectId: string; stages: Stage[] } = $props();

	type Rule = { id: string; stage_id: string; kind: string; params: Record<string, any> };
	type Data = { rules: Rule[]; triggers: { event: string; stage_id: string }[]; log: { task_id: string | null; number: number | null; title: string | null; what: string; ok: boolean; at: string }[] };
	let data = $state<Data | null>(null);
	$effect(() => {
		if (open) api.get<Data>(`/projects/${projectId}/automation`).then((d) => (data = d));
	});

	const KINDS: Record<string, string> = {
		notify: 'Tell someone',
		comment: 'Add a comment',
		set_responsible: 'Make someone responsible',
		add_assistant: 'Add an assistant',
		add_watcher: 'Add a watcher',
		set_deadline: 'Set the deadline',
		set_priority: 'Set the priority',
		set_status: 'Set the status',
		create_subtask: 'Make a subtask'
	};
	const EVENTS: Record<string, string> = {
		comment: 'Someone comments',
		overdue: 'The deadline passes',
		checklist_done: 'The checklist is all ticked',
		started: 'Work starts'
	};
	const TO: Record<string, string> = { responsible: 'the responsible person', creator: 'whoever set the task', owners: 'the project owners', participants: 'everyone on the task' };

	let adding = $state<string | null>(null);
	let draft = $state<{ kind: string; text: string; to: string; user_id: string | null; user_name: string | null; days: number; hours: number; priority: string; status: string; title: string; responsible: string }>(blank());
	function blank() {
		return { kind: 'notify', text: '', to: 'responsible', user_id: null, user_name: null, days: 1, hours: 0, priority: 'high', status: 'in_progress', title: '', responsible: 'responsible' };
	}
	function params() {
		const d = draft;
		switch (d.kind) {
			case 'notify':
				return { to: d.to, text: d.text };
			case 'comment':
				return { text: d.text };
			case 'set_responsible':
			case 'add_assistant':
			case 'add_watcher':
				return { user_id: d.user_id };
			case 'set_deadline':
				return { days: Number(d.days), hours: Number(d.hours) };
			case 'set_priority':
				return { priority: d.priority };
			case 'set_status':
				return { status: d.status };
			default:
				return { title: d.title, responsible: d.responsible };
		}
	}
	async function addRule(e: SubmitEvent) {
		e.preventDefault();
		try {
			data = await api.post<Data>(`/projects/${projectId}/automation/rules`, { stage_id: adding, kind: draft.kind, params: params() });
			adding = null;
			draft = blank();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeRule(r: Rule) {
		data = await api.del<Data>(`/projects/${projectId}/automation/rules/${r.id}`);
	}
	async function setTrigger(event: string, stage: string) {
		try {
			data = stage ? await api.post<Data>(`/projects/${projectId}/automation/triggers`, { event, stage_id: stage }) : await api.del<Data>(`/projects/${projectId}/automation/triggers/${event}`);
		} catch (err) {
			toasts.error(err);
		}
	}
	function describe(r: Rule) {
		const p = r.params;
		switch (r.kind) {
			case 'notify':
				return `Tell ${TO[p.to] ?? 'someone'}: “${p.text}”`;
			case 'comment':
				return `Comment: “${p.text}”`;
			case 'set_deadline':
				return `Deadline in ${p.days ? `${p.days} d` : ''}${p.hours ? ` ${p.hours} h` : ''}`;
			case 'set_priority':
				return `Priority: ${p.priority}`;
			case 'set_status':
				return `Status: ${STATUS[p.status as keyof typeof STATUS]?.label ?? p.status}`;
			case 'create_subtask':
				return `Subtask “${p.title}”`;
			default:
				return KINDS[r.kind];
		}
	}
</script>

<Modal bind:open title="Automation" size="lg">
	{#if data}
		<p class="mb-3 text-sm text-slate-500">Rules run when a task enters their column. Texts may use {'{task}'}, {'{number}'}, {'{column}'} and {'{responsible}'}.</p>
		<div class="space-y-3" aria-label="Rules">
			{#each stages as s (s.id)}
				{@const rules = data.rules.filter((r) => r.stage_id === s.id)}
				<section class="rounded-lg border border-slate-200" data-automation-stage={s.name}>
					<header class="flex items-center gap-2 border-b border-slate-100 px-3 py-2">
						<h3 class="flex-1 text-sm font-semibold text-slate-800">{s.name}</h3>
						<button class="text-xs text-brand-600 hover:underline" onclick={() => ((adding = s.id), (draft = blank()))}>Add rule</button>
					</header>
					<ul class="divide-y divide-slate-100 text-sm">
						{#each rules as r (r.id)}
							<li class="flex items-center gap-2 px-3 py-1.5" data-rule={r.kind}><span class="flex-1">{describe(r)}</span><button class="text-slate-400 hover:text-red-600" aria-label="Remove rule" onclick={() => removeRule(r)}><Icon name="x" class="size-4" /></button></li>
						{:else}
							{#if adding !== s.id}<li class="px-3 py-1.5 text-xs text-slate-400">No rules.</li>{/if}
						{/each}
					</ul>
					{#if adding === s.id}
						<form class="space-y-2 border-t border-slate-100 bg-slate-50 p-3" onsubmit={addRule}>
							<select class="input" aria-label="Rule" bind:value={draft.kind}>{#each Object.entries(KINDS) as [k, l] (k)}<option value={k}>{l}</option>{/each}</select>
							{#if draft.kind === 'notify'}
								<select class="input" aria-label="Whom" bind:value={draft.to}>{#each Object.entries(TO) as [k, l] (k)}<option value={k}>{l}</option>{/each}</select>
							{/if}
							{#if draft.kind === 'notify' || draft.kind === 'comment'}
								<input class="input" placeholder="Text" aria-label="Text" required bind:value={draft.text} />
							{:else if ['set_responsible', 'add_assistant', 'add_watcher'].includes(draft.kind)}
								<UserPicker bind:value={draft.user_id} bind:label={draft.user_name} />
							{:else if draft.kind === 'set_deadline'}
								<div class="flex items-center gap-2 text-sm">In <input type="number" min="0" max="365" class="input w-20" aria-label="Days" bind:value={draft.days} /> days <input type="number" min="0" max="23" class="input w-20" aria-label="Hours" bind:value={draft.hours} /> hours</div>
							{:else if draft.kind === 'set_priority'}
								<select class="input" aria-label="Priority" bind:value={draft.priority}><option value="high">High</option><option value="normal">Normal</option><option value="low">Low</option></select>
							{:else if draft.kind === 'set_status'}
								<select class="input" aria-label="Status" bind:value={draft.status}>{#each Object.entries(STATUS) as [k, v] (k)}<option value={k}>{v.label}</option>{/each}</select>
							{:else if draft.kind === 'create_subtask'}
								<input class="input" placeholder="Subtask title" aria-label="Subtask title" required bind:value={draft.title} />
								<select class="input" aria-label="Subtask for" bind:value={draft.responsible}><option value="responsible">For the responsible person</option><option value="creator">For whoever set the task</option></select>
							{/if}
							<div class="flex gap-2"><button class="btn btn-primary">Add</button><button type="button" class="btn btn-ghost" onclick={() => (adding = null)}>Cancel</button></div>
						</form>
					{/if}
				</section>
			{/each}
		</div>
		<h3 class="mt-5 mb-2 text-sm font-semibold text-slate-800">Triggers: move the task to a column when…</h3>
		<div class="grid gap-2 sm:grid-cols-2" aria-label="Triggers">
			{#each Object.entries(EVENTS) as [ev, label] (ev)}
				<label class="text-sm">
					<span class="mb-1 block text-slate-600">{label}</span>
					<select class="input" aria-label="Trigger: {label}" value={data.triggers.find((t) => t.event === ev)?.stage_id ?? ''} onchange={(e) => setTrigger(ev, e.currentTarget.value)}>
						<option value="">— nothing —</option>
						{#each stages as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
					</select>
				</label>
			{/each}
		</div>
		{#if data.log.length}
			<h3 class="mt-5 mb-2 text-sm font-semibold text-slate-800">Recently</h3>
			<ul class="max-h-48 space-y-1 overflow-y-auto text-xs" aria-label="Automation log">
				{#each data.log as l, i (i)}
					<li class="flex gap-2 {l.ok ? 'text-slate-600' : 'text-red-600'}"><span class="text-slate-400">{formatDateTime(l.at)}</span>{#if l.task_id}<a class="text-brand-600 hover:underline" href="/tasks/{l.task_id}">#{l.number}</a>{/if}<span>{l.what}</span></li>
				{/each}
			</ul>
		{/if}
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Close</button>
	{/snippet}
</Modal>
