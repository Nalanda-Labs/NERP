<script lang="ts">
	// An attachment (on a task, a chat message or a post) open in Collabora Online: the editor runs
	// in a frame, opened by posting a signed token to it; saves update the attachment everywhere.
	import { page } from '$app/state';
	import { onMount, tick } from 'svelte';
	import { api } from '#lib/api.ts';

	type Opened = { url: string; token: string; token_ttl: number; action: string; name: string; can_edit: boolean };
	const id = $derived(page.params.id!);
	let opened = $state<Opened | null>(null);
	let error = $state('');
	let form: HTMLFormElement | undefined = $state();

	onMount(async () => {
		try {
			opened = await api.post<Opened>(`/files/${id}/office`, { view: page.url.searchParams.get('view') === '1' });
			await tick();
			form?.submit();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	});
</script>

<svelte:head><title>{opened?.name ?? 'Document'} · NERP</title></svelte:head>

<div class="flex h-[calc(100dvh-7rem)] min-h-[480px] flex-col gap-2">
	<div class="flex items-center gap-2">
		<a class="btn btn-ghost px-2" href="/" aria-label="Back" onclick={(e) => { if (history.length > 1) { e.preventDefault(); history.back(); } }}>‹</a>
		<h1 class="text-lg font-semibold text-slate-900">{opened?.name ?? ''}</h1>
		{#if opened && !opened.can_edit}<span class="badge bg-slate-100 text-slate-600">View only</span>{/if}
		{#if opened?.can_edit}<span class="text-xs text-slate-500">Saves update the attachment wherever it is shown; earlier versions are kept.</span>{/if}
	</div>
	{#if error}
		<p class="card p-6 text-sm text-red-700" role="alert">{error}</p>
	{:else if opened}
		<form bind:this={form} action={opened.url} method="post" target="office-frame" class="hidden">
			<input type="hidden" name="access_token" value={opened.token} />
			<input type="hidden" name="access_token_ttl" value={opened.token_ttl} />
		</form>
		<iframe name="office-frame" title="Document editor" class="card w-full flex-1" allow="clipboard-read; clipboard-write; fullscreen" data-office></iframe>
	{:else}
		<p class="text-sm text-slate-400">Opening…</p>
	{/if}
</div>
