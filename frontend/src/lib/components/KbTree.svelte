<script lang="ts">
	// The pages of a knowledge base space, as a tree.
	import KbTree from './KbTree.svelte';

	type Node = { id: string; parent_id: string | null; title: string };
	let { pages, parent = null, current = null }: { pages: Node[]; parent?: string | null; current?: string | null } = $props();
	const here = $derived(pages.filter((p) => p.parent_id === parent));
</script>

{#if here.length}
	<ul class={parent ? 'ml-3 border-l border-slate-100 pl-2' : ''}>
		{#each here as p (p.id)}
			<li>
				<a href="/kb/page/{p.id}" class="block truncate rounded-lg px-2 py-1 text-sm {current === p.id ? 'bg-brand-50 font-medium text-brand-700' : 'text-slate-700 hover:bg-slate-50'}" data-tree={p.title}>{p.title}</a>
				<KbTree {pages} parent={p.id} {current} />
			</li>
		{/each}
	</ul>
{/if}
