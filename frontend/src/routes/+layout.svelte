<script lang="ts">
	import './layout.css';
	import favicon from '#lib/assets/favicon.svg';
	import Toaster from '#lib/components/Toaster.svelte';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>NERP</title>
</svelte:head>

{@render children()}
<Toaster />
