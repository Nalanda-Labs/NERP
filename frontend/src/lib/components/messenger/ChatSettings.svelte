<script lang="ts">
	// A chat's details and settings: name, description, who can find it and who writes,
	// automatic deletion, people and their roles, muting, clearing history, leaving.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import { AUTO_DELETE, type ChatDetail, type Person } from '#lib/messenger.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Avatar from '../Avatar.svelte';
	import Modal from '../Modal.svelte';
	import MultiUserPicker from '../MultiUserPicker.svelte';

	let {
		open = $bindable(false),
		chat,
		me,
		onchange,
		onleft,
		oncleared
	}: {
		open?: boolean;
		chat: ChatDetail;
		me: string | undefined;
		onchange: (c: ChatDetail) => void;
		onleft: () => void;
		oncleared: () => void;
	} = $props();

	let form = $state({ title: '', description: '', visibility: 'private', post_policy: 'all', auto_delete_hours: '' as string });
	let adding = $state<Person[]>([]);
	$effect(() => {
		if (!open) return;
		untrack(() => {
			form = { title: chat.title, description: chat.description ?? '', visibility: chat.visibility, post_policy: chat.post_policy, auto_delete_hours: chat.auto_delete_hours == null ? '' : String(chat.auto_delete_hours) };
			adding = [];
		});
	});
	const isOwner = $derived(chat.my_role === 'owner');
	const fixed = $derived(chat.audience !== 'manual' || !!chat.project_id || !!chat.task_id || !!chat.event_id);

	async function patch(body: Record<string, unknown>) {
		try {
			onchange(await api.patch<ChatDetail>(`/im/chats/${chat.id}`, body));
			return true;
		} catch (e) {
			toasts.error(e);
			return false;
		}
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		const body: Record<string, unknown> = {};
		if (!fixed) {
			if (form.title !== chat.title) body.title = form.title;
			if (form.description !== (chat.description ?? '')) body.description = form.description || null;
			if (form.visibility !== chat.visibility && !chat.is_general) body.visibility = form.visibility;
			if (form.post_policy !== chat.post_policy && chat.kind !== 'channel') body.post_policy = form.post_policy;
		} else if (chat.audience !== 'manual') {
			if (form.description !== (chat.description ?? '')) body.description = form.description || null;
			if (form.post_policy !== chat.post_policy && chat.kind !== 'channel') body.post_policy = form.post_policy;
		}
		const hours = form.auto_delete_hours === '' ? null : Number(form.auto_delete_hours);
		if (hours !== chat.auto_delete_hours) body.auto_delete_hours = hours;
		if (!Object.keys(body).length || (await patch(body))) open = false;
	}
	async function addPeople() {
		if (adding.length && (await patch({ add: adding.map((p) => p.id) }))) adding = [];
	}
	async function removePerson(p: Person) {
		if (confirm(`Remove ${p.name} from the chat?`)) await patch({ remove: [p.id] });
	}
	async function setRole(p: Person, role: string) {
		if (role === 'owner' && !confirm(`Hand the chat over to ${p.name}? You become an admin.`)) return;
		try {
			onchange(await api.put<ChatDetail>(`/im/chats/${chat.id}/members/${p.id}`, { role }));
		} catch (e) {
			toasts.error(e);
		}
	}
	async function mute() {
		try {
			onchange(await api.patch<ChatDetail>(`/im/chats/${chat.id}/me`, { muted: !chat.muted }));
		} catch (e) {
			toasts.error(e);
		}
	}
	async function clear(everyone: boolean) {
		if (!confirm(everyone ? 'Delete every message in this chat, for everyone? This cannot be undone.' : 'Clear the history for yourself? Others still see it.')) return;
		try {
			await api.post(`/im/chats/${chat.id}/clear`, { everyone });
			open = false;
			oncleared();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function leave() {
		if (!confirm(chat.kind === 'channel' ? 'Leave this channel?' : 'Leave this chat?')) return;
		try {
			await api.post(`/im/chats/${chat.id}/leave`, {});
			open = false;
			onleft();
		} catch (e) {
			toasts.error(e);
		}
	}
	const ROLE = { owner: 'Owner', admin: 'Admin', member: 'Member' };
</script>

<Modal bind:open title={chat.kind === 'channel' ? 'Channel' : 'Chat'} size="lg">
	<form id="chat-settings" class="space-y-4" onsubmit={save}>
		{#if chat.kind !== 'direct'}
			<p class="text-xs text-slate-500" data-chat-kind>
				{chat.kind === 'channel' ? 'Channel' : 'Group chat'} ·
				{#if chat.is_general}everyone in the company{:else if chat.audience === 'department'}the {chat.department_name} department{:else if chat.project_id || chat.task_id || chat.event_id}follows its workgroup, task or event{:else}{chat.visibility === 'open' ? 'open: anyone can find and join it' : 'private: people are added'}{/if}
			</p>
		{/if}
		{#if chat.can.manage && chat.kind !== 'direct'}
			{#if !fixed}
				<div><label class="label" for="cs-title">Name</label><input id="cs-title" class="input" required maxlength="120" bind:value={form.title} /></div>
			{/if}
			{#if !chat.project_id && !chat.task_id && !chat.event_id}
				<div><label class="label" for="cs-desc">Description</label><textarea id="cs-desc" class="input" rows="2" bind:value={form.description}></textarea></div>
			{/if}
			<div class="grid gap-3 sm:grid-cols-3">
				{#if !fixed}
					<div>
						<label class="label" for="cs-vis">Who finds it</label>
						<select id="cs-vis" class="input" bind:value={form.visibility}><option value="private">Only those added</option><option value="open">Everyone (open)</option></select>
					</div>
				{/if}
				{#if chat.kind !== 'channel' && !chat.project_id && !chat.task_id && !chat.event_id}
					<div>
						<label class="label" for="cs-post">Who writes</label>
						<select id="cs-post" class="input" bind:value={form.post_policy}><option value="all">Everyone</option><option value="managers">Owner and admins</option></select>
					</div>
				{/if}
				<div>
					<label class="label" for="cs-del">Delete messages</label>
					<select id="cs-del" class="input" bind:value={form.auto_delete_hours}>
						{#each AUTO_DELETE as [h, label] (label)}<option value={h == null ? '' : String(h)}>{label}</option>{/each}
					</select>
				</div>
			</div>
		{:else if chat.description}
			<p class="text-sm text-slate-600">{chat.description}</p>
		{/if}

		{#if chat.kind !== 'direct'}
			<div>
				<h3 class="mb-1 text-sm font-medium text-slate-700">{chat.members} {chat.members === 1 ? 'person' : 'people'}</h3>
				<ul class="max-h-64 divide-y divide-slate-100 overflow-y-auto">
					{#each chat.people as p (p.id)}
						<li class="flex items-center gap-3 py-2" data-person={p.name}>
							<Avatar name={p.name} fileId={p.avatar_file_id} size="sm" />
							<span class="flex-1 text-sm">{p.name}</span>
							{#if isOwner && p.id !== me && !chat.project_id && !chat.task_id && !chat.event_id}
								<select class="input w-28 py-1 text-xs" aria-label="Role of {p.name}" value={p.role} onchange={(e) => setRole(p, e.currentTarget.value)}>
									<option value="member">Member</option><option value="admin">Admin</option><option value="owner">Owner</option>
								</select>
							{:else if p.role && p.role !== 'member'}
								<span class="badge bg-slate-100 text-slate-600">{ROLE[p.role]}</span>
							{/if}
							{#if chat.can.edit_people && p.id !== me && p.role !== 'owner'}<button type="button" class="text-xs text-slate-400 hover:text-red-600" onclick={() => removePerson(p)}>Remove</button>{/if}
						</li>
					{/each}
				</ul>
			</div>
			{#if chat.can.add_people}
				<div class="flex gap-2"><div class="flex-1"><MultiUserPicker bind:value={adding} exclude={chat.people.map((p) => p.id)} placeholder="Add people…" /></div><button type="button" class="btn btn-secondary" onclick={addPeople}>Add</button></div>
			{/if}
		{/if}

		<div class="flex flex-wrap gap-2 border-t border-slate-100 pt-4">
			{#if chat.my_role}
				<button type="button" class="btn btn-secondary" onclick={mute} data-mute>{chat.muted ? 'Unmute' : 'Mute'}</button>
				<button type="button" class="btn btn-ghost" onclick={() => clear(false)}>Clear history for me</button>
				{#if chat.can.manage && chat.kind !== 'direct'}<button type="button" class="btn btn-ghost text-red-600" onclick={() => clear(true)}>Delete history for everyone</button>{/if}
				{#if chat.can.leave}<button type="button" class="btn btn-ghost ml-auto text-red-600" onclick={leave}>Leave</button>{/if}
			{/if}
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Close</button>
		{#if chat.can.manage && chat.kind !== 'direct'}<button class="btn btn-primary" form="chat-settings">Save</button>{/if}
	{/snippet}
</Modal>
