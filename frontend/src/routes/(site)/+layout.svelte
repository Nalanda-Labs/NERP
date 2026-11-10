<script lang="ts">
	// Websites made with the builder: no app chrome, no sign-in.
	import type { LayoutProps } from './$types';
	let { children }: LayoutProps = $props();
</script>

{@render children()}
