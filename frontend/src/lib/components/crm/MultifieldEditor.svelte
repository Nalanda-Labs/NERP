<script lang="ts">
	import { crm, optionLabel, type Multifield } from '#lib/crm.svelte.ts';
	import Icon from '../Icon.svelte';

	let { value = $bindable<Multifield[]>([]), kinds = ['phone', 'email', 'web', 'im'] }: { value?: Multifield[]; kinds?: Multifield['kind'][] } = $props();
	const LABELS: Record<string, string> = { phone: 'Phone', email: 'Email', web: 'Website', im: 'Messenger' };
	const PLACEHOLDER: Record<string, string> = { phone: '+1 555 010 7788', email: 'name@example.com', web: 'https://example.com', im: '@username' };
	const types = (k: string) => crm.config?.options[k] ?? ['work'];

	function add(kind: Multifield['kind']) {
		value = [...value, { kind, value_type: types(kind)[0]!, value: '' }];
	}
</script>

<div class="space-y-3">
	{#each kinds as kind (kind)}
		{@const rows = value.filter((m) => m.kind === kind)}
		<div>
			<span class="label">{LABELS[kind]}</span>
			<div class="space-y-2">
				{#each rows as m (m)}
					<div class="flex gap-2">
						<input class="input" type={kind === 'email' ? 'email' : kind === 'phone' ? 'tel' : 'text'} placeholder={PLACEHOLDER[kind]} bind:value={m.value} />
						<select class="input w-36" bind:value={m.value_type} aria-label="{LABELS[kind]} type">
							{#each types(kind) as t (t)}<option value={t}>{optionLabel(t)}</option>{/each}
						</select>
						<button type="button" class="btn btn-ghost px-2 text-slate-400 hover:text-red-600" aria-label="Remove" onclick={() => (value = value.filter((x) => x !== m))}><Icon name="x" class="size-4" /></button>
					</div>
				{/each}
			</div>
			<button type="button" class="mt-1 text-sm font-medium text-brand-600 hover:underline" onclick={() => add(kind)}>+ Add {LABELS[kind]!.toLowerCase()}</button>
		</div>
	{/each}
</div>
