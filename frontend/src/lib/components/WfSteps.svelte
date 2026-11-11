<script module lang="ts">
	export type ListField = { id?: string; code: string; label: string; kind: string; options: string[]; required: boolean };
	export type Who = { users: string[]; author: boolean; head: boolean };
	export type Step = {
		kind: 'approval' | 'condition' | 'notify' | 'task' | 'set_field' | 'wait' | 'stop';
		title?: string;
		who?: Who;
		mode?: 'any' | 'all';
		due_hours?: number;
		approved?: Step[];
		rejected?: Step[];
		field?: string;
		op?: string;
		value?: string;
		then?: Step[];
		else?: Step[];
		text?: string;
		description?: string;
		deadline_hours?: number;
		/** A task step: wait here until its tasks are done. */
		wait?: boolean;
		hours?: number;
		status?: 'completed' | 'rejected';
	};
	export const STEP_NAMES: Record<Step['kind'], string> = {
		approval: 'Approval',
		condition: 'If…',
		notify: 'Notify',
		task: 'Task',
		set_field: 'Set a field',
		wait: 'Wait',
		stop: 'Stop'
	};
	export function newStep(kind: Step['kind'], fields: ListField[]): Step {
		const who = (): Who => ({ users: [], author: false, head: true });
		switch (kind) {
			case 'approval':
				return { kind, title: 'Approve {{title}}', who: who(), mode: 'any', approved: [], rejected: [] };
			case 'condition':
				return { kind, field: fields[0]?.code ?? '', op: 'eq', value: '', then: [], else: [] };
			case 'notify':
				return { kind, who: { users: [], author: true, head: false }, text: '{{title}}: ' };
			case 'task':
				return { kind, who: who(), title: 'Handle {{title}}', description: '', deadline_hours: 24 };
			case 'set_field':
				return { kind, field: fields[0]?.code ?? '', value: '' };
			case 'wait':
				return { kind, hours: 24 };
			default:
				return { kind, status: 'completed' };
		}
	}
</script>

<script lang="ts">
	// A list of process steps; branches (approval outcomes, conditions) nest the same editor.
	import Icon from '#lib/components/Icon.svelte';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import WfSteps from './WfSteps.svelte';

	type Person = { id: string; name: string; avatar_file_id: string | null };
	let { steps = $bindable(), fields, people = $bindable({}), depth = 0, author = 'author' }: { steps: Step[]; fields: ListField[]; people?: Record<string, Person>; depth?: number; /** What “the author” is called (a CRM record's responsible person). */ author?: string } = $props();
	let adding = $state<Step['kind']>('approval');
	const OPS: [string, string][] = [['eq', 'is'], ['ne', 'is not'], ['gt', 'more than'], ['lt', 'less than'], ['empty', 'is empty'], ['filled', 'is filled']];
	function move(i: number, d: number) {
		const j = i + d;
		if (j < 0 || j >= steps.length) return;
		[steps[i], steps[j]] = [steps[j], steps[i]];
	}
	// The people picker works with names; steps keep ids.
	const picked = (w: Who) => w.users.map((id) => people[id] ?? { id, name: '…', avatar_file_id: null });
	function setPicked(w: Who, list: Person[]) {
		for (const p of list) people[p.id] = p;
		w.users = list.map((p) => p.id);
	}
</script>

{#snippet whoEditor(w: Who)}
	<div class="space-y-1.5">
		<div class="flex flex-wrap gap-3 text-xs">
			<label class="flex items-center gap-1"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={w.author} /> The {author}</label>
			<label class="flex items-center gap-1"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={w.head} /> The {author}'s head</label>
		</div>
		<MultiUserPicker value={picked(w)} placeholder="And these people…" onchange={(v: Person[]) => setPicked(w, v)} />
	</div>
{/snippet}

<ol class="space-y-2 {depth ? 'border-l-2 border-slate-200 pl-3' : ''}">
	{#each steps as st, i (i)}
		<li class="rounded-xl border border-slate-200 bg-white p-3" data-step={st.kind}>
			<div class="mb-2 flex items-center gap-2">
				<span class="text-xs font-semibold tracking-wide text-slate-500 uppercase">{i + 1}. {STEP_NAMES[st.kind]}</span>
				<span class="ml-auto flex gap-1">
					<button type="button" class="btn btn-ghost p-1" aria-label="Move up" onclick={() => move(i, -1)}>↑</button>
					<button type="button" class="btn btn-ghost p-1" aria-label="Move down" onclick={() => move(i, 1)}>↓</button>
					<button type="button" class="btn btn-ghost p-1 hover:text-red-600" aria-label="Remove step" onclick={() => steps.splice(i, 1)}><Icon name="trash" class="size-4" /></button>
				</span>
			</div>
			{#if st.kind === 'approval'}
				<input class="input mb-2" aria-label="What to approve" bind:value={st.title} />
				{@render whoEditor(st.who!)}
				<div class="mt-2 flex flex-wrap items-center gap-3 text-sm">
					<select class="input w-auto" aria-label="Who must approve" bind:value={st.mode}><option value="any">Any one of them</option><option value="all">All of them</option></select>
					<label class="flex items-center gap-1">Within <input class="input w-20" type="number" min="1" placeholder="—" bind:value={st.due_hours} /> hours</label>
				</div>
				<div class="mt-3 grid gap-3 lg:grid-cols-2">
					<div><p class="mb-1 text-xs font-medium text-emerald-700">If approved</p><WfSteps bind:steps={st.approved!} {fields} bind:people depth={depth + 1} {author} /></div>
					<div><p class="mb-1 text-xs font-medium text-red-700">If rejected {st.rejected?.length ? '' : '(ends as rejected)'}</p><WfSteps bind:steps={st.rejected!} {fields} bind:people depth={depth + 1} {author} /></div>
				</div>
			{:else if st.kind === 'condition'}
				<div class="flex flex-wrap gap-2">
					<select class="input w-auto" aria-label="Field" bind:value={st.field}>{#each fields as f (f.code)}<option value={f.code}>{f.label}</option>{/each}</select>
					<select class="input w-auto" aria-label="Comparison" bind:value={st.op}>{#each OPS as [v, l] (v)}<option value={v}>{l}</option>{/each}</select>
					{#if st.op !== 'empty' && st.op !== 'filled'}<input class="input w-40" aria-label="Value" bind:value={st.value} />{/if}
				</div>
				<div class="mt-3 grid gap-3 lg:grid-cols-2">
					<div><p class="mb-1 text-xs font-medium text-slate-600">Then</p><WfSteps bind:steps={st.then!} {fields} bind:people depth={depth + 1} {author} /></div>
					<div><p class="mb-1 text-xs font-medium text-slate-600">Otherwise</p><WfSteps bind:steps={st.else!} {fields} bind:people depth={depth + 1} {author} /></div>
				</div>
			{:else if st.kind === 'notify'}
				{@render whoEditor(st.who!)}
				<input class="input mt-2" aria-label="Notification text" bind:value={st.text} />
			{:else if st.kind === 'task'}
				{@render whoEditor(st.who!)}
				<input class="input mt-2" aria-label="Task title" bind:value={st.title} />
				<textarea class="input mt-2" rows="2" aria-label="Task description" bind:value={st.description}></textarea>
				<div class="mt-2 flex flex-wrap items-center gap-4 text-sm">
					<label class="flex items-center gap-1">Due in <input class="input w-20" type="number" min="1" bind:value={st.deadline_hours} /> hours</label>
					<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={st.wait} data-wait-for-task /> Wait until the task is done</label>
				</div>
			{:else if st.kind === 'set_field'}
				<div class="flex gap-2">
					<select class="input w-auto" aria-label="Field to set" bind:value={st.field}>{#each fields.filter((f) => !['file', 'user', 'crm'].includes(f.kind)) as f (f.code)}<option value={f.code}>{f.label}</option>{/each}</select>
					{#if fields.find((f) => f.code === st.field)?.kind === 'select'}
						<select class="input w-auto" aria-label="New value" bind:value={st.value}><option value="">(empty)</option>{#each fields.find((f) => f.code === st.field)!.options as o (o)}<option>{o}</option>{/each}</select>
					{:else}<input class="input flex-1" aria-label="New value" bind:value={st.value} />{/if}
				</div>
			{:else if st.kind === 'wait'}
				<label class="flex items-center gap-1 text-sm">Wait <input class="input w-24" type="number" min="0.02" step="any" bind:value={st.hours} /> hours</label>
			{:else}
				<select class="input w-auto" aria-label="End as" bind:value={st.status}><option value="completed">End as completed</option><option value="rejected">End as rejected</option></select>
			{/if}
		</li>
	{/each}
	<li class="flex gap-2">
		<select class="input w-auto text-sm" aria-label="Step to add" bind:value={adding}>{#each Object.entries(STEP_NAMES) as [k, n] (k)}<option value={k}>{n}</option>{/each}</select>
		<button type="button" class="btn btn-secondary px-3 py-1 text-sm" onclick={() => steps.push(newStep(adding, fields))}>Add step</button>
	</li>
</ol>
