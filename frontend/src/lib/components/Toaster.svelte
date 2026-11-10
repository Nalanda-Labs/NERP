<script lang="ts">
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from './Icon.svelte';
</script>

<div class="pointer-events-none fixed right-4 bottom-4 z-50 flex w-80 flex-col gap-2" aria-live="polite">
	{#each toasts.items as t (t.id)}
		<div
			class="pointer-events-auto flex items-start gap-3 rounded-lg border px-4 py-3 text-sm shadow-lg
				{t.kind === 'error'
				? 'border-red-200 bg-red-50 text-red-800'
				: t.kind === 'info'
					? 'border-slate-200 bg-white text-slate-800'
					: 'border-emerald-200 bg-emerald-50 text-emerald-800'}"
		>
			<span class="flex-1">{t.message}</span>
			<button class="opacity-60 hover:opacity-100" onclick={() => toasts.dismiss(t.id)} aria-label="Dismiss">
				<Icon name="x" class="size-4" />
			</button>
		</div>
	{/each}
</div>
