<script lang="ts">
	// A task or a calendar event made from a chat or one of its messages.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import type { Msg } from '#lib/messenger.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Modal from '../Modal.svelte';
	import UserPicker from '../UserPicker.svelte';

	let {
		open = $bindable(false),
		what,
		chatId,
		message = null
	}: { open?: boolean; what: 'task' | 'event'; chatId: string; message?: Msg | null } = $props();

	const tomorrow = () => new Date(Date.now() + 86400e3).toISOString().slice(0, 10);
	let form = $state({ title: '', description: '', responsible: null as string | null, responsibleName: null as string | null, deadline: '', date: '', time: '10:00', minutes: 30, invite: true });
	$effect(() => {
		if (!open) return;
		untrack(() => {
			const first = message?.body.split('\n')[0]?.slice(0, 200) ?? '';
			form = { title: first, description: '', responsible: null, responsibleName: null, deadline: '', date: tomorrow(), time: '10:00', minutes: 30, invite: true };
		});
	});

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		try {
			if (what === 'task') {
				const r = await api.post<{ task_id: string; number: number }>(`/im/chats/${chatId}/task`, {
					title: form.title || null,
					description: form.description || null,
					responsible_id: form.responsible,
					deadline: form.deadline ? new Date(form.deadline + 'T18:00:00').toISOString() : null,
					message_id: message?.id ?? null
				});
				open = false;
				toasts.success(`Task #${r.number} made`);
			} else {
				const start = new Date(`${form.date}T${form.time}:00`);
				await api.post(`/im/chats/${chatId}/event`, {
					title: form.title || null,
					description: form.description || null,
					start: start.toISOString(),
					end: new Date(+start + form.minutes * 60e3).toISOString(),
					timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
					message_id: message?.id ?? null,
					invite: form.invite
				});
				open = false;
				toasts.success('The event is on the calendar');
			}
		} catch (err) {
			toasts.error(err);
		}
	}
</script>

<Modal bind:open title={what === 'task' ? 'Make a task' : 'Plan an event'}>
	<form id="make-from-chat" class="space-y-3" onsubmit={submit}>
		{#if message}<blockquote class="border-l-2 border-slate-300 pl-3 text-sm whitespace-pre-line text-slate-600">{message.body}</blockquote>{/if}
		<div><label class="label" for="mk-title">Title</label><input id="mk-title" class="input" required maxlength="200" bind:value={form.title} /></div>
		<div><label class="label" for="mk-desc">Notes</label><textarea id="mk-desc" class="input" rows="2" bind:value={form.description}></textarea></div>
		{#if what === 'task'}
			<div class="grid gap-3 sm:grid-cols-2">
				<div><span class="label">Responsible (you if empty)</span><UserPicker bind:value={form.responsible} bind:label={form.responsibleName} placeholder="Choose…" /></div>
				<div><label class="label" for="mk-dl">Deadline</label><input id="mk-dl" type="date" class="input" bind:value={form.deadline} /></div>
			</div>
		{:else}
			<div class="grid gap-3 sm:grid-cols-3">
				<div><label class="label" for="mk-date">On</label><input id="mk-date" type="date" class="input" required bind:value={form.date} /></div>
				<div><label class="label" for="mk-time">At</label><input id="mk-time" type="time" class="input" required bind:value={form.time} /></div>
				<div><label class="label" for="mk-min">Minutes</label><input id="mk-min" type="number" min="5" max="1440" class="input" required bind:value={form.minutes} /></div>
			</div>
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.invite} /> Invite everyone in the chat</label>
		{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="make-from-chat">{what === 'task' ? 'Make the task' : 'Add to calendar'}</button>
	{/snippet}
</Modal>
