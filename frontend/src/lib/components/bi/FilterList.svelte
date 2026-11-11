<script lang="ts">
	// Conditions on columns: column, condition and value(s), as fits the column's type.
	import { needsValue, opsFor, type Col, type Filter } from '#lib/bi.ts';

	let {
		cols,
		filters = $bindable([]),
		label = 'Only rows where'
	}: { cols: Col[]; filters: Filter[]; label?: string } = $props();

	const typeOf = (f: Filter) => cols.find((c) => c.key === f.field)?.type ?? 'text';
	function add() {
		const c = cols.find((x) => x.type === 'text') ?? cols[0];
		if (c)
			filters = [
				...filters,
				{ field: c.key, op: opsFor(c.type)[0][0], value: c.type === 'bool' ? true : '' }
			];
	}
	function setField(i: number, key: string) {
		const c = cols.find((x) => x.key === key)!;
		filters[i] = { field: key, op: opsFor(c.type)[0][0], value: c.type === 'bool' ? true : '' };
	}
	function setOp(i: number, op: string) {
		const f = filters[i];
		filters[i] = {
			...f,
			op,
			value: op === 'between' ? ['', ''] : op === 'in' ? [] : Array.isArray(f.value) ? '' : f.value
		};
	}
	const input = (t: string) =>
		t === 'number' ? 'number' : t === 'date' || t === 'time' ? 'date' : 'text';
</script>

<div class="space-y-2" data-filters>
	{#if filters.length}<p class="text-xs text-slate-500">{label}</p>{/if}
	{#each filters as f, i (i)}
		{@const t = typeOf(f)}
		<div class="flex flex-wrap items-center gap-1">
			<select
				class="input w-auto py-1"
				aria-label="Column"
				value={f.field}
				onchange={(e) => setField(i, e.currentTarget.value)}
			>
				{#each cols as c (c.key)}<option value={c.key}>{c.label}</option>{/each}
			</select>
			<select
				class="input w-auto py-1"
				aria-label="Condition"
				value={f.op}
				onchange={(e) => setOp(i, e.currentTarget.value)}
			>
				{#each opsFor(t) as [k, l] (k)}<option value={k}>{l}</option>{/each}
			</select>
			{#if needsValue(f.op)}
				{#if t === 'bool'}
					<select
						class="input w-auto py-1"
						aria-label="Value"
						value={String(f.value)}
						onchange={(e) => (filters[i].value = e.currentTarget.value === 'true')}
						><option value="true">yes</option><option value="false">no</option></select
					>
				{:else if f.op === 'between'}
					{@const v = (f.value as string[]) ?? ['', '']}
					<input
						class="input w-32 py-1"
						type={input(t)}
						aria-label="From"
						value={v[0]}
						oninput={(e) => (filters[i].value = [e.currentTarget.value, v[1]])}
					/>
					<span class="text-slate-400">and</span>
					<input
						class="input w-32 py-1"
						type={input(t)}
						aria-label="To"
						value={v[1]}
						oninput={(e) => (filters[i].value = [v[0], e.currentTarget.value])}
					/>
				{:else if f.op === 'in'}
					<input
						class="input w-56 py-1"
						aria-label="Values, separated by commas"
						placeholder="a, b, c"
						value={((f.value as string[]) ?? []).join(', ')}
						oninput={(e) =>
							(filters[i].value = e.currentTarget.value
								.split(',')
								.map((s) => s.trim())
								.filter(Boolean))}
					/>
				{:else}
					<input
						class="input w-40 py-1"
						type={input(t)}
						aria-label="Value"
						value={f.value as string}
						oninput={(e) => (filters[i].value = e.currentTarget.value)}
					/>
				{/if}
			{/if}
			<button
				class="px-1 text-slate-400 hover:text-red-600"
				aria-label="Remove condition"
				onclick={() => (filters = filters.filter((_, j) => j !== i))}>×</button
			>
		</div>
	{/each}
	<button
		class="text-sm text-brand-600 hover:underline"
		type="button"
		onclick={add}
		disabled={!cols.length}>+ Add a condition</button
	>
</div>
