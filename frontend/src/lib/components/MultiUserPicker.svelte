<script lang="ts" module>
	export interface PickedUser {
		id: string;
		name: string;
		avatar_file_id: string | null;
	}
</script>

<script lang="ts">
	import Avatar from './Avatar.svelte';
	import Icon from './Icon.svelte';
	import UserPicker from './UserPicker.svelte';

	let {
		value = $bindable<PickedUser[]>([]),
		exclude = [],
		placeholder = 'Add people…',
		onchange
	}: { value?: PickedUser[]; exclude?: string[]; placeholder?: string; /** Called with the new list after each change. */ onchange?: (v: PickedUser[]) => void } = $props();
	function set(v: PickedUser[]) {
		value = v;
		onchange?.(v);
	}

	let pickerKey = $state(0);
</script>

<div class="space-y-2">
	{#if value.length}
		<div class="flex flex-wrap gap-1.5">
			{#each value as u (u.id)}
				<span class="inline-flex items-center gap-1.5 rounded-full bg-slate-100 py-0.5 pr-1 pl-0.5 text-sm text-slate-700">
					<Avatar name={u.name} fileId={u.avatar_file_id} size="xs" />
					{u.name}
					<button type="button" class="rounded-full p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-700" aria-label="Remove {u.name}" onclick={() => set(value.filter((x) => x.id !== u.id))}>
						<Icon name="x" class="size-3" />
					</button>
				</span>
			{/each}
		</div>
	{/if}
	{#key pickerKey}
		<UserPicker
			{placeholder}
			exclude={[...exclude, ...value.map((v) => v.id)]}
			onpick={(u) => {
				set([...value, { id: u.id, name: u.full_name, avatar_file_id: u.avatar_file_id }]);
				pickerKey++;
			}}
		/>
	{/key}
</div>
