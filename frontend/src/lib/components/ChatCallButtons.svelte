<script lang="ts">
	// "Chat" and "📹 Call" for a person, workgroup, task or event: `chat` finds (or makes) its
	// chat; Call starts the chat's call, or joins the one already going on.
	import { goto } from '$app/navigation';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from './Icon.svelte';

	let {
		chat,
		chatLabel = 'Chat',
		primary = false,
		small = false,
		call = true
	}: { chat: () => Promise<string>; chatLabel?: string; primary?: boolean; small?: boolean; /** false: nobody else to call (yet). */ call?: boolean } = $props();
	let busy = $state(false);

	async function go(call: boolean) {
		if (busy) return;
		busy = true;
		try {
			const id = await chat();
			if (call) {
				const c = await api.post<{ id: string }>('/calls', { chat_id: id });
				await goto(`/call/${c.id}`);
			} else await goto(`/messenger?chat=${id}`);
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
</script>

<button class="btn {primary ? 'btn-primary' : 'btn-secondary'} {small ? 'text-sm' : ''}" disabled={busy} onclick={() => go(false)}>
	<Icon name="message" class="size-4" /> {chatLabel}
</button>
{#if call}<button class="btn btn-secondary {small ? 'text-sm' : ''}" disabled={busy} onclick={() => go(true)} data-call-button>📹 Call</button>{/if}
