<script lang="ts">
	// Public web forms: no app chrome, no sign-in.
	import type { LayoutProps } from './$types';
	let { children }: LayoutProps = $props();
</script>

{@render children()}
