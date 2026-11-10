<script lang="ts">
	// A textarea that offers colleagues when you type "@". The text shows "@Full Name";
	// `encoded` carries the same text with picked mentions as "@[Full Name](user-id)",
	// which is what gets sent so the server can notify those people.
	import { untrack } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import type { Page, User } from '#lib/types.ts';
	import Avatar from './Avatar.svelte';

	let {
		value = $bindable(''),
		encoded = $bindable(''),
		placeholder = 'Write a comment… Type @ to mention someone',
		rows = 3,
		onsubmit
	}: { value?: string; encoded?: string; placeholder?: string; rows?: number; onsubmit?: () => void } = $props();

	// Display name -> user id for everyone picked in this text.
	let picked = $state<Record<string, string>>({});

	$effect(() => {
		let out = value;
		// Longest names first so "Ann Lee" wins over "Ann".
		for (const name of Object.keys(picked).sort((a, b) => b.length - a.length)) {
			out = out.split(`@${name}`).join(`@[${name}](${picked[name]})`);
		}
		encoded = out;
	});

	// Forget picked people once the box is cleared (e.g. after sending).
	$effect(() => {
		if (!value) untrack(() => Object.keys(picked).length && (picked = {}));
	});

	let el: HTMLTextAreaElement | undefined = $state();
	let results = $state<User[]>([]);
	let active = $state(0);
	let mention = $state<{ start: number; term: string } | null>(null);
	let timer: ReturnType<typeof setTimeout>;

	function detect() {
		if (!el) return;
		const upto = value.slice(0, el.selectionStart);
		const m = /(^|\s)@([\p{L}\p{N}._-]{0,30})$/u.exec(upto);
		if (!m) {
			mention = null;
			results = [];
			return;
		}
		mention = { start: upto.length - m[2]!.length - 1, term: m[2]! };
		clearTimeout(timer);
		timer = setTimeout(async () => {
			const p = await api.get<Page<User>>('/users' + query({ q: mention?.term, limit: 6 }));
			results = p.items;
			active = 0;
		}, 150);
	}

	function pick(u: User) {
		if (!mention || !el) return;
		picked[u.full_name] = u.id;
		const token = `@${u.full_name} `;
		const end = mention.start + 1 + mention.term.length;
		value = value.slice(0, mention.start) + token + value.slice(end);
		const caret = mention.start + token.length;
		mention = null;
		results = [];
		queueMicrotask(() => {
			el!.focus();
			el!.setSelectionRange(caret, caret);
		});
	}

	function onkeydown(e: KeyboardEvent) {
		if (mention && results.length) {
			if (e.key === 'ArrowDown') {
				e.preventDefault();
				active = (active + 1) % results.length;
				return;
			}
			if (e.key === 'ArrowUp') {
				e.preventDefault();
				active = (active - 1 + results.length) % results.length;
				return;
			}
			if (e.key === 'Enter' || e.key === 'Tab') {
				e.preventDefault();
				pick(results[active]!);
				return;
			}
			if (e.key === 'Escape') {
				mention = null;
				results = [];
				return;
			}
		}
		if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
			e.preventDefault();
			onsubmit?.();
		}
	}
</script>

<div class="relative">
	<textarea
		bind:this={el}
		bind:value
		{rows}
		{placeholder}
		class="input resize-y"
		oninput={detect}
		onclick={detect}
		{onkeydown}
		onblur={() => setTimeout(() => (results = []), 150)}
	></textarea>
	{#if mention && results.length}
		<ul class="absolute bottom-full left-0 z-20 mb-1 w-72 overflow-hidden rounded-xl bg-white py-1 shadow-g2">
			{#each results as u, i (u.id)}
				<li>
					<button
						type="button"
						class="flex w-full items-center gap-2.5 px-3 py-2 text-left text-sm {i === active ? 'bg-brand-50' : 'hover:bg-slate-50'}"
						onmousedown={(e) => {
							e.preventDefault();
							pick(u);
						}}
					>
						<Avatar name={u.full_name} fileId={u.avatar_file_id} size="xs" />
						<span class="truncate font-medium text-slate-800">{u.full_name}</span>
						<span class="truncate text-xs text-slate-400">{u.position ?? ''}</span>
					</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>
