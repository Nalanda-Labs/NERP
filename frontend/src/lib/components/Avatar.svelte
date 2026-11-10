<script lang="ts">
	import { fileUrl } from '#lib/api.ts';
	import { initials } from '#lib/format.ts';

	let {
		name,
		fileId = null,
		size = 'md'
	}: { name: string; fileId?: string | null; size?: 'xs' | 'sm' | 'md' | 'lg' | 'xl' } = $props();

	const sizes = {
		xs: 'size-6 text-[10px]',
		sm: 'size-8 text-xs',
		md: 'size-10 text-sm',
		lg: 'size-14 text-lg',
		xl: 'size-24 text-3xl'
	};
	// A stable pastel per name so the same person always gets the same colour.
	const palette = [
		'bg-sky-100 text-sky-700',
		'bg-emerald-100 text-emerald-700',
		'bg-amber-100 text-amber-700',
		'bg-rose-100 text-rose-700',
		'bg-violet-100 text-violet-700',
		'bg-teal-100 text-teal-700',
		'bg-orange-100 text-orange-700',
		'bg-indigo-100 text-indigo-700'
	];
	const color = $derived(
		palette[[...name].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) % palette.length]
	);
	let broken = $state(false);
</script>

{#if fileId && !broken}
	<img
		src={fileUrl(fileId)}
		alt={name}
		class="{sizes[size]} shrink-0 rounded-full object-cover ring-1 ring-black/5"
		onerror={() => (broken = true)}
	/>
{:else}
	<span
		class="{sizes[size]} {color} inline-flex shrink-0 items-center justify-center rounded-full font-semibold"
		aria-label={name}
	>
		{initials(name)}
	</span>
{/if}
