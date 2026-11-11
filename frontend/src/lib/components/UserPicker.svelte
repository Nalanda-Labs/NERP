<script lang="ts">
	import { api, query } from '#lib/api.ts';
	import type { Page, User } from '#lib/types.ts';
	import Avatar from './Avatar.svelte';

	let {
		value = $bindable<string | null>(null),
		label = $bindable<string | null>(null),
		placeholder = 'Search employees…',
		exclude = [],
		onpick
	}: {
		value?: string | null;
		label?: string | null;
		placeholder?: string;
		exclude?: string[];
		onpick?: (u: User) => void;
	} = $props();

	let term = $state('');
	let results = $state<User[]>([]);
	let open = $state(false);
	let active = $state(0);
	// What `results` were found for (so Enter never picks from an older search).
	let resultsFor = $state('');
	const listId = `up-${Math.random().toString(36).slice(2)}`;
	let timer: ReturnType<typeof setTimeout>;
	let seq = 0;
	// Enter pressed before the results came back: pick once they do.
	let pickWhenReady = false;

	/** `pasted`: a whole name pasted in is picked when exactly one person has it. */
	function search(pasted = false) {
		clearTimeout(timer);
		const n = ++seq;
		timer = setTimeout(async () => {
			const q = term.trim();
			const page = await api.get<Page<User>>('/users' + query({ with_guests: true, q, limit: 8 }));
			if (n !== seq) return;
			results = page.items.filter((u) => !exclude.includes(u.id));
			resultsFor = q;
			active = 0;
			open = true;
			const exact = results.filter((u) => u.full_name.toLowerCase() === q.toLowerCase());
			if (pickWhenReady && results.length) pick(exact[0] ?? results[0]);
			else if (pasted && exact.length === 1) pick(exact[0]);
			pickWhenReady = false;
		}, pasted ? 0 : 200);
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
			if (!results.length) return;
			e.preventDefault();
			open = true;
			active = (active + (e.key === 'ArrowDown' ? 1 : results.length - 1)) % results.length;
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (!term.trim()) return;
			// Results for what's typed now, or wait for them.
			if (open && results.length && resultsFor === term.trim()) pick(results[active] ?? results[0]);
			else pickWhenReady = true;
		} else if (e.key === 'Escape' && open) {
			// Close the list, not the dialog around it.
			e.preventDefault();
			e.stopPropagation();
			open = false;
		}
	}

	function pick(u: User) {
		value = u.id;
		label = u.full_name;
		term = '';
		open = false;
		onpick?.(u);
	}
</script>

<div class="relative">
	<input
		class="input"
		{placeholder}
		value={open || !label ? term : label}
		role="combobox"
		aria-expanded={open && results.length > 0}
		aria-autocomplete="list"
		aria-controls={listId}
		oninput={(e) => {
			term = e.currentTarget.value;
			search((e as unknown as InputEvent).inputType === 'insertFromPaste');
		}}
		{onkeydown}
		onfocus={() => search()}
		onblur={() => setTimeout(() => (open = false), 150)}
	/>
	{#if open && results.length}
		<ul id={listId} role="listbox" class="absolute z-20 mt-1 max-h-64 w-full overflow-auto rounded-xl bg-white py-1 shadow-g2">
			{#each results as u, i (u.id)}
				<li role="option" aria-selected={i === active}>
					<button
						type="button"
						tabindex="-1"
						class="flex w-full items-center gap-3 px-3 py-2 text-left text-sm hover:bg-slate-50 {i === active ? 'bg-slate-50' : ''}"
						onmousedown={() => pick(u)}
					>
						<Avatar name={u.full_name} fileId={u.avatar_file_id} size="sm" />
						<span class="min-w-0">
							<span class="block truncate font-medium text-slate-800">{u.full_name}</span>
							<span class="block truncate text-xs text-slate-500">{u.position ?? u.email}</span>
						</span>
					</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>
