<script lang="ts">
	// A business process as a flowchart: steps top to bottom, branches (approved / rejected,
	// yes / no) side by side. Steps are added where the "+" is and removed with "×"; their
	// details are set in the step editor.
	import { newStep, STEP_NAMES, type ListField, type Step } from './WfSteps.svelte';
	import WfFlow from './WfFlow.svelte';

	let { steps = $bindable(), fields, depth = 0, author = 'author' }: { steps: Step[]; fields: ListField[]; depth?: number; author?: string } = $props();
	let menuAt = $state<number | null>(null);
	const OPS: Record<string, string> = { eq: '=', ne: '≠', gt: '>', lt: '<', empty: 'is empty', filled: 'is filled' };
	const COLORS: Record<Step['kind'], string> = {
		approval: 'border-amber-300 bg-amber-50',
		condition: 'border-violet-300 bg-violet-50',
		notify: 'border-sky-300 bg-sky-50',
		task: 'border-emerald-300 bg-emerald-50',
		set_field: 'border-slate-300 bg-slate-50',
		wait: 'border-slate-300 bg-white',
		stop: 'border-red-300 bg-red-50'
	};
	const label = (code?: string) => fields.find((f) => f.code === code)?.label ?? code ?? '';
	const who = (w?: Step['who']) => w ? [w.author && `the ${author}`, w.head && `the ${author}'s head`, w.users.length && `${w.users.length} ${w.users.length === 1 ? 'person' : 'people'}`].filter(Boolean).join(', ') : '';
	function summary(st: Step): string {
		switch (st.kind) {
			case 'approval': return `${st.title} — ${who(st.who)}${st.mode === 'all' ? ' (all)' : ''}`;
			case 'condition': return `${label(st.field)} ${OPS[st.op ?? 'eq']} ${st.op === 'empty' || st.op === 'filled' ? '' : `“${st.value}”`}`;
			case 'notify': return `${who(st.who)}: ${st.text}`;
			case 'task': return `${st.title} — ${who(st.who)}${st.wait ? ', waits for it' : ''}`;
			case 'set_field': return `${label(st.field)} → “${st.value}”`;
			case 'wait': return `${st.hours} hours`;
			default: return st.status === 'rejected' ? 'as rejected' : 'as completed';
		}
	}
	function insert(at: number, kind: Step['kind']) {
		steps.splice(at, 0, newStep(kind, fields));
		menuAt = null;
	}
</script>

{#snippet adder(at: number)}
	<div class="relative flex flex-col items-center">
		<span class="h-3 w-px bg-slate-300" aria-hidden="true"></span>
		<button type="button" class="flex size-6 items-center justify-center rounded-full border border-slate-300 bg-white text-sm text-slate-500 hover:border-brand-500 hover:text-brand-700" aria-label="Add a step here" onclick={() => (menuAt = menuAt === at ? null : at)} data-flow-add>+</button>
		{#if menuAt === at}
			<div class="absolute top-9 z-10 w-40 rounded-xl border border-slate-200 bg-white p-1 shadow-lg" role="menu">
				{#each Object.entries(STEP_NAMES) as [k, n] (k)}<button type="button" role="menuitem" class="block w-full rounded-lg px-2 py-1 text-left text-xs hover:bg-slate-50" onclick={() => insert(at, k as Step['kind'])}>{n}</button>{/each}
			</div>
		{/if}
		<span class="h-3 w-px bg-slate-300" aria-hidden="true"></span>
	</div>
{/snippet}

<div class="flex flex-col items-center" data-flow-depth={depth}>
	{#if depth === 0}<div class="rounded-full bg-slate-900 px-4 py-1 text-xs font-medium text-white">Start</div>{/if}
	{@render adder(0)}
	{#each steps as st, i (i)}
		<div class="group relative w-64 max-w-full rounded-xl border-2 px-3 py-2 text-xs {COLORS[st.kind]}" data-flow-node={st.kind}>
			<p class="font-semibold text-slate-800">{STEP_NAMES[st.kind]}</p>
			<p class="line-clamp-2 text-slate-600">{summary(st)}</p>
			<button type="button" class="absolute top-1 right-1.5 hidden text-slate-400 group-hover:block hover:text-red-600" aria-label="Remove this step" onclick={() => steps.splice(i, 1)}>×</button>
		</div>
		{#if st.kind === 'approval' || st.kind === 'condition'}
			{@const [a, b, la, lb] = st.kind === 'approval' ? (['approved', 'rejected', 'Approved', 'Rejected'] as const) : (['then', 'else', 'Yes', 'No'] as const)}
			<span class="h-3 w-px bg-slate-300" aria-hidden="true"></span>
			<div class="grid grid-cols-2 gap-4 rounded-xl border border-dashed border-slate-200 p-2">
				<div class="flex flex-col items-center"><span class="text-[11px] font-medium {a === 'approved' ? 'text-emerald-700' : 'text-slate-600'}">{la}</span><WfFlow bind:steps={st[a]!} {fields} depth={depth + 1} {author} /></div>
				<div class="flex flex-col items-center"><span class="text-[11px] font-medium {b === 'rejected' ? 'text-red-700' : 'text-slate-600'}">{lb}{b === 'rejected' && !st.rejected?.length ? ' (ends)' : ''}</span><WfFlow bind:steps={st[b]!} {fields} depth={depth + 1} {author} /></div>
			</div>
		{/if}
		{@render adder(i + 1)}
	{/each}
	{#if depth === 0}<div class="rounded-full bg-slate-900 px-4 py-1 text-xs font-medium text-white">End</div>{/if}
</div>
