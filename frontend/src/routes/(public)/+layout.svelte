<script lang="ts">
	import type { LayoutProps } from './$types';
	let { children }: LayoutProps = $props();
</script>

<div class="flex min-h-screen items-center justify-center bg-gradient-to-br from-brand-50 via-white to-slate-100 px-4 py-12">
	<div class="w-full max-w-md">
		<div class="mb-8 flex items-center justify-center gap-2.5">
			<span class="flex size-10 items-center justify-center rounded-xl bg-brand-600 text-lg font-bold text-white shadow-sm">N</span>
			<span class="text-2xl font-semibold tracking-tight text-slate-900">NERP</span>
		</div>
		{@render children()}
	</div>
</div>
