<script lang="ts">
	import type { Snippet } from 'svelte';
	import Icon from './Icon.svelte';

	let {
		open = $bindable(false),
		title,
		size = 'md',
		children,
		footer,
		onclose
	}: {
		open?: boolean;
		title: string;
		size?: 'sm' | 'md' | 'lg';
		children: Snippet;
		footer?: Snippet;
		onclose?: () => void;
	} = $props();

	function close() {
		open = false;
		onclose?.();
	}

	let dialog: HTMLDialogElement | undefined = $state();
	const widths = { sm: 'max-w-sm', md: 'max-w-lg', lg: 'max-w-2xl' };

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) dialog.showModal();
		if (!open && dialog.open) dialog.close();
	});
</script>

<dialog
	bind:this={dialog}
	onclose={close}
	onclick={(e) => e.target === dialog && close()}
	class="m-auto w-[calc(100%-2rem)] {widths[size]} rounded-[28px] bg-white p-0 shadow-g2 backdrop:bg-slate-900/40 backdrop:backdrop-blur-[1px]"
>
	{#if open}
		<div class="flex items-center justify-between px-6 pt-6 pb-2">
			<h2 class="text-xl text-slate-900">{title}</h2>
			<button class="btn btn-ghost -mr-2 p-1.5" onclick={close} aria-label="Close">
				<Icon name="x" />
			</button>
		</div>
		<div class="max-h-[70vh] overflow-y-auto px-6 py-3">
			{@render children()}
		</div>
		{#if footer}
			<div class="flex justify-end gap-2 px-6 pt-2 pb-6">
				{@render footer()}
			</div>
		{/if}
	{/if}
</dialog>
