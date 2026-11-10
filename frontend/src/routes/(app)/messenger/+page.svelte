<script lang="ts">
	// Messenger: one-to-one and group chats, channels, the general chat and department chats,
	// open chats to find and join, folders, live.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount, tick } from 'svelte';
	import { api, query, streamUrl } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import ChatSettings from '#lib/components/messenger/ChatSettings.svelte';
	import Composer from '#lib/components/messenger/Composer.svelte';
	import MakeFromChat from '#lib/components/messenger/MakeFromChat.svelte';
	import MessageItem from '#lib/components/messenger/MessageItem.svelte';
	import { formatRelative } from '#lib/format.ts';
	import type { Chat, ChatDetail, Folder, Msg, MsgFile, Person } from '#lib/messenger.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM, type Department } from '#lib/types.ts';

	// Videos (call recordings) play here, streamed so they can be seeked.
	let player = $state<{ name: string; url: string } | null>(null);
	async function play(f: MsgFile) {
		try {
			player = { name: f.name, url: await streamUrl(`/api/files/${f.id}/content`) };
		} catch (e) {
			toasts.error(e);
		}
	}

	let chats = $state<Chat[]>([]);
	let folders = $state<Folder[]>([]);
	/** all | unread | channel | a folder id */
	let filter = $state('all');
	let q = $state('');
	let current = $state<ChatDetail | null>(null);
	let msgs = $state<Msg[]>([]);
	let more = $state(false);
	let list: HTMLElement | undefined = $state();
	const selected = $derived(page.url.searchParams.get('chat'));
	const me = $derived(session.user?.id);

	async function loadChats() {
		const f = filter;
		chats = (
			await api.get<{ items: Chat[] }>(
				'/im/chats' +
					query({
						q: q.trim() || undefined,
						unread: f === 'unread' || undefined,
						kind: f === 'channel' ? 'channel' : undefined,
						folder_id: !['all', 'unread', 'channel'].includes(f) ? f : undefined
					})
			)
		).items;
	}
	async function loadFolders() {
		folders = (await api.get<{ items: Folder[] }>('/im/folders')).items;
	}
	async function bottom() {
		await tick();
		list?.scrollTo({ top: list.scrollHeight });
	}
	async function loadMessages(id: string) {
		const r = await api.get<{ items: Msg[]; more: boolean }>(`/im/chats/${id}/messages`);
		msgs = r.items;
		more = r.more;
	}
	async function open(id: string | null) {
		thread = null;
		if (!id) return void (current = null);
		try {
			current = await api.get<ChatDetail>(`/im/chats/${id}`);
			await loadMessages(id);
			await bottom();
			markRead();
		} catch {
			current = null;
		}
	}
	async function markRead(force = false) {
		if (!current?.my_role) return;
		const c = chats.find((x) => x.id === current!.id);
		if (force || c?.unread || !c) {
			await api.post(`/im/chats/${current.id}/read`, {});
			if (c) c.unread = 0;
		}
	}
	async function older() {
		if (!current || !msgs.length) return;
		const h = list!.scrollHeight;
		const r = await api.get<{ items: Msg[]; more: boolean }>(`/im/chats/${current.id}/messages?before=${encodeURIComponent(msgs[0]!.created_at)}`);
		msgs = [...r.items, ...msgs];
		more = r.more;
		await tick();
		list!.scrollTop = list!.scrollHeight - h;
	}
	function put(m: Msg) {
		if (m.parent_id) {
			if (thread?.post.id === m.parent_id) {
				const i = thread.items.findIndex((x) => x.id === m.id);
				if (i >= 0) thread.items[i] = m;
				else thread.items = [...thread.items, m];
			}
			return false;
		}
		if (thread?.post.id === m.id) thread.post = m;
		const i = msgs.findIndex((x) => x.id === m.id);
		if (i >= 0) {
			msgs[i] = m;
			return false;
		}
		msgs = [...msgs, m];
		return true;
	}
	async function refetch(id: string) {
		try {
			put(await api.get<Msg>(`/im/messages/${id}`));
		} catch {
			/* gone */
		}
	}
	onMount(() => {
		loadChats();
		loadFolders();
		const off = [
			realtime.on('im.message', async (d: { chat_id: string; message: Msg }) => {
				if (d.chat_id === current?.id) {
					// Broadcast copies carry nobody's own votes and reactions: take mine.
					const added = d.message.poll || d.message.reactions.length ? (await refetch(d.message.id), !msgs.some((x) => x.id === d.message.id)) : put(d.message);
					if (added && !d.message.parent_id) {
						const near = list ? list.scrollHeight - list.scrollTop - list.clientHeight < 200 : true;
						if (near || d.message.user_id === me) bottom();
						if (d.message.user_id !== me && document.visibilityState === 'visible') await markRead(true);
					}
				}
				loadChats();
			}),
			realtime.on('im.message.changed', (d: { chat_id: string; message_id: string }) => {
				if (d.chat_id === current?.id) refetch(d.message_id);
			}),
			realtime.on('im.chat', async (d: { chat_id: string; cleared?: boolean }) => {
				loadChats();
				loadFolders();
				if (d.chat_id === current?.id) {
					try {
						current = await api.get<ChatDetail>(`/im/chats/${d.chat_id}`);
						if (d.cleared) await loadMessages(d.chat_id);
					} catch {
						current = null;
					}
				}
			}),
			realtime.on('im.read', (d: { chat_id: string; user_id: string; at: string }) => {
				if (d.chat_id === current?.id) {
					const p = current.people.find((x) => x.id === d.user_id);
					if (p) p.last_read_at = d.at;
				}
			})
		];
		return () => off.forEach((f) => f());
	});
	$effect(() => {
		open(selected);
	});
	const pick = (id: string) => goto(`/messenger?chat=${id}`, { replace: true });
	function setFilter(f: string) {
		filter = f;
		loadChats();
	}

	function sent(m: Msg) {
		if (put(m)) bottom();
		loadChats();
	}
	async function join() {
		try {
			current = await api.post<ChatDetail>(`/im/chats/${current!.id}/join`);
			loadChats();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function startCall() {
		try {
			const c = await api.post<{ id: string }>('/calls', { chat_id: current!.id });
			goto(`/call/${c.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function openGeneral() {
		try {
			const c = await api.get<ChatDetail>('/im/general');
			await loadChats();
			pick(c.id);
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- comments under a channel post -----
	let thread = $state<{ post: Msg; items: Msg[] } | null>(null);
	async function openThread(m: Msg) {
		const r = await api.get<{ items: Msg[] }>(`/im/chats/${m.chat_id}/messages?parent_id=${m.id}&limit=200`);
		thread = { post: m, items: r.items };
	}

	// ----- tasks and events from the chat -----
	let make = $state<{ what: 'task' | 'event'; message: Msg | null } | null>(null);
	let makeOpen = $state(false);
	function startMake(what: 'task' | 'event', message: Msg | null = null) {
		make = { what, message };
		makeOpen = true;
	}

	// ----- new chats -----
	let newOpen = $state(false);
	let newKind = $state<'direct' | 'group' | 'channel'>('direct');
	let form = $state({ title: '', description: '', visibility: 'private', audience: 'manual', department_id: '' });
	let groupPeople = $state<Person[]>([]);
	let departments = $state<Department[]>([]);
	// One person: choose them, then start a chat or a video call.
	let directWith = $state<{ id: string; full_name?: string; name?: string } | null>(null);
	async function openNew() {
		form = { title: '', description: '', visibility: 'private', audience: 'manual', department_id: '' };
		groupPeople = [];
		newOpen = true;
		const all = await api.get<Department[]>('/departments').catch(() => [] as Department[]);
		departments = session.can(PERM.DEPARTMENTS_MANAGE) ? all : all.filter((d) => d.head_user_id === me);
	}
	async function startDirect(call = false) {
		if (!directWith) return;
		try {
			const c = await api.post<Chat>(`/im/direct/${directWith.id}`, {});
			newOpen = false;
			directWith = null;
			if (call) {
				const k = await api.post<{ id: string }>('/calls', { chat_id: c.id });
				return goto(`/call/${k.id}`);
			}
			await loadChats();
			pick(c.id);
		} catch (err) {
			toasts.error(err);
		}
	}
	async function startGroup(e: SubmitEvent) {
		e.preventDefault();
		try {
			const c = await api.post<Chat>('/im/chats', {
				kind: newKind,
				title: form.title,
				description: form.description || null,
				visibility: form.visibility,
				audience: form.audience,
				department_id: form.audience === 'department' ? form.department_id : null,
				members: form.audience === 'manual' ? groupPeople.map((p) => p.id) : []
			});
			newOpen = false;
			await loadChats();
			pick(c.id);
		} catch (err) {
			toasts.error(err);
		}
	}

	// ----- open chats to join -----
	type Found = { id: string; kind: 'group' | 'channel'; title: string; description: string | null; members: number };
	let findOpen = $state(false);
	let findQ = $state('');
	let found = $state<Found[]>([]);
	async function find() {
		found = (await api.get<{ items: Found[] }>('/im/open' + query({ q: findQ.trim() || undefined }))).items;
	}

	// ----- folders -----
	let folderOpen = $state(false);
	let folderForm = $state<{ id: string | null; name: string; chat_ids: string[] }>({ id: null, name: '', chat_ids: [] });
	let allChats = $state<Chat[]>([]);
	async function editFolder(f: Folder | null) {
		folderForm = f ? { id: f.id, name: f.name, chat_ids: [...f.chat_ids] } : { id: null, name: '', chat_ids: current ? [current.id] : [] };
		allChats = (await api.get<{ items: Chat[] }>('/im/chats')).items;
		folderOpen = true;
	}
	async function saveFolder(e: SubmitEvent) {
		e.preventDefault();
		try {
			const body = { name: folderForm.name, chat_ids: folderForm.chat_ids };
			folders = (folderForm.id ? await api.patch<{ items: Folder[] }>(`/im/folders/${folderForm.id}`, body) : await api.post<{ items: Folder[] }>('/im/folders', body)).items;
			folderOpen = false;
			loadChats();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function deleteFolder() {
		if (!folderForm.id || !confirm('Delete this folder? The chats stay.')) return;
		folders = (await api.del<{ items: Folder[] }>(`/im/folders/${folderForm.id}`)).items;
		folderOpen = false;
		if (filter === folderForm.id) setFilter('all');
	}

	let settingsOpen = $state(false);
	let t: ReturnType<typeof setTimeout> | undefined;
	const search = () => (clearTimeout(t), (t = setTimeout(loadChats, 250)));
	const dayOf = (iso: string) => new Date(iso).toDateString();
	/** Read by everyone else, for my latest message. */
	const seenBy = (m: Msg) => current?.people.filter((p) => p.id !== me && p.last_read_at && p.last_read_at >= m.created_at).map((p) => p.name.split(' ')[0]!) ?? [];
	const icon = (c: Chat) => (c.kind === 'channel' ? '📣' : c.is_general ? '🏢' : c.audience === 'department' ? '🏷️' : null);
</script>

<svelte:head><title>Messenger · NERP</title></svelte:head>

<div class="flex h-[calc(100dvh-8rem)] min-h-[32rem] gap-4">
	<section class="card flex w-80 shrink-0 flex-col" aria-label="Chats">
		<div class="flex items-center gap-2 px-4 pt-4">
			<h1 class="flex-1 text-lg font-semibold text-slate-900">Messenger</h1>
			<a class="btn btn-ghost px-2 py-1.5 text-sm" href="/calls" title="Call history" aria-label="Call history"><Icon name="clock" class="size-4" /></a>
			<button class="btn btn-ghost px-2 py-1.5 text-sm" title="Find open chats and channels" aria-label="Find open chats" onclick={() => ((findOpen = true), find())}><Icon name="search" class="size-4" /></button>
			<button class="btn btn-primary px-3 py-1.5 text-sm" onclick={openNew}><Icon name="plus" class="size-4" /> New</button>
		</div>
		<div class="px-3 pt-3"><input class="input" type="search" placeholder="Find a chat…" aria-label="Find a chat" bind:value={q} oninput={search} /></div>
		<div class="flex gap-1 overflow-x-auto px-3 py-2 text-xs" role="tablist" aria-label="Show">
			{#each [['all', 'All'], ['unread', 'Unread'], ['channel', 'Channels']] as [k, l] (k)}
				<button role="tab" aria-selected={filter === k} class="shrink-0 rounded-full px-2.5 py-1 font-medium {filter === k ? 'bg-brand-600 text-white' : 'bg-slate-100 text-slate-600'}" onclick={() => setFilter(k!)}>{l}</button>
			{/each}
			{#each folders as f (f.id)}
				<button role="tab" aria-selected={filter === f.id} class="shrink-0 rounded-full px-2.5 py-1 font-medium {filter === f.id ? 'bg-brand-600 text-white' : 'bg-slate-100 text-slate-600'}" onclick={() => setFilter(f.id)} ondblclick={() => editFolder(f)} title="Double-click to edit" data-folder={f.name}>{f.name}{f.unread ? ` · ${f.unread}` : ''}</button>
			{/each}
			<button class="shrink-0 rounded-full bg-slate-100 px-2 py-1 text-slate-500" aria-label="New folder" title="New folder" onclick={() => editFolder(null)}>+</button>
		</div>
		<ul class="flex-1 overflow-y-auto">
			{#each chats as c (c.id)}
				<li>
					<button class="flex w-full items-center gap-3 px-4 py-2.5 text-left hover:bg-slate-50 {current?.id === c.id ? 'bg-brand-50' : ''}" onclick={() => pick(c.id)} data-chat={c.title}>
						{#if c.kind === 'direct'}<Avatar name={c.title} fileId={c.avatar_file_id} size="md" />{:else}<span class="flex size-9 items-center justify-center rounded-full bg-slate-200 text-slate-600">{#if icon(c)}{icon(c)}{:else}<Icon name="users" class="size-4" />{/if}</span>{/if}
						<span class="min-w-0 flex-1">
							<span class="flex items-center gap-2"><span class="flex-1 truncate font-medium text-slate-900">{c.title}</span>{#if c.muted}<span class="text-xs text-slate-400" title="Muted">🔕</span>{/if}<span class="text-xs text-slate-400">{formatRelative(c.last_message_at)}</span></span>
							<span class="flex items-center gap-2"><span class="flex-1 truncate text-sm text-slate-500">{c.last_author && c.kind !== 'direct' ? `${c.last_author}: ` : ''}{c.last_body ?? ''}</span>{#if c.unread}<span class="rounded-full px-1.5 text-xs font-semibold text-white {c.muted ? 'bg-slate-400' : 'bg-brand-600'}" data-unread>{c.unread}</span>{/if}</span>
						</span>
					</button>
				</li>
			{:else}
				<li class="px-4 py-10 text-center text-sm text-slate-500">{filter === 'all' ? 'No chats yet. Start one with “New”.' : 'Nothing here.'}</li>
			{/each}
		</ul>
		<button class="border-t border-slate-100 px-4 py-2.5 text-left text-sm text-slate-600 hover:bg-slate-50" onclick={openGeneral} data-general>🏢 General chat</button>
	</section>

	<section class="card flex min-w-0 flex-1 flex-col" aria-label="Conversation">
		{#if current}
			<div class="flex items-center gap-3 border-b border-slate-100 px-5 py-3">
				<div class="min-w-0 flex-1">
					<h2 class="truncate font-semibold text-slate-900">{current.kind === 'channel' ? '📣 ' : ''}{current.title}</h2>
					{#if current.description}<p class="truncate text-xs text-slate-500">{current.description}</p>{/if}
				</div>
				{#if current.task_id}<a class="text-sm text-brand-600 hover:underline" href="/tasks/{current.task_id}" data-chat-of="task">Open task</a>
				{:else if current.event_id}<a class="text-sm text-brand-600 hover:underline" href="/calendar?event={current.event_id}" data-chat-of="event">Open event</a>
				{:else if current.project_id}<a class="text-sm text-brand-600 hover:underline" href="/projects/{current.project_id}" data-chat-of="workgroup">Open workgroup</a>{/if}
				{#if current.my_role}
					<button class="btn btn-ghost px-2 py-1" title="Make a task" aria-label="Make a task from the chat" onclick={() => startMake('task')}><Icon name="checkSquare" class="size-4" /></button>
					<button class="btn btn-ghost px-2 py-1" title="Plan an event" aria-label="Plan an event from the chat" onclick={() => startMake('event')}><Icon name="calendar" class="size-4" /></button>
				{/if}
				<button class="btn btn-ghost px-2 py-1 text-sm" onclick={() => (settingsOpen = true)} data-chat-info>{current.kind === 'direct' ? 'Info' : `${current.members} ${current.members === 1 ? 'person' : 'people'}`}</button>
				{#if current.my_role && current.kind !== 'channel'}<button class="btn btn-secondary px-3 py-1 text-sm" onclick={startCall} data-call-button>📹 Call</button>{/if}
			</div>
			<div class="flex min-h-0 flex-1">
				<div class="flex min-w-0 flex-1 flex-col">
					<div bind:this={list} class="flex-1 space-y-1 overflow-y-auto px-5 py-4" data-messages>
						{#if more}<div class="text-center"><button class="text-xs text-brand-600 hover:underline" onclick={older}>Earlier messages</button></div>{/if}
						{#each msgs as m, i (m.id)}
							{#if i === 0 || dayOf(msgs[i - 1]!.created_at) !== dayOf(m.created_at)}
								<p class="py-2 text-center text-xs text-slate-400">{new Date(m.created_at).toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' })}</p>
							{/if}
							<MessageItem
								{m}
								chat={current}
								{me}
								first={i === 0 || msgs[i - 1]!.user_id !== m.user_id || dayOf(msgs[i - 1]!.created_at) !== dayOf(m.created_at) || current.kind === 'channel'}
								seen={m.user_id === me && i === msgs.length - 1 ? seenBy(m) : []}
								onchange={put}
								oncomments={openThread}
								onplay={play}
								onmake={startMake}
							/>
						{/each}
					</div>
					{#if !current.my_role}
						<div class="flex items-center gap-3 border-t border-slate-100 p-3" data-preview>
							<span class="flex-1 text-sm text-slate-600">You're reading an open {current.kind === 'channel' ? 'channel' : 'chat'}.</span>
							<button class="btn btn-primary" onclick={join}>Join</button>
						</div>
					{:else if current.can.post}
						<Composer chatId={current.id} onsent={sent} onfocus={() => markRead()} />
					{:else}
						<p class="border-t border-slate-100 p-3 text-center text-sm text-slate-500" data-read-only>{current.kind === 'channel' ? 'Only the channel\'s managers post here. You can comment on posts.' : 'Only the chat\'s owner and admins write here.'}</p>
					{/if}
				</div>
				{#if thread}
					<aside class="flex w-96 shrink-0 flex-col border-l border-slate-100" aria-label="Comments">
						<div class="flex items-center border-b border-slate-100 px-4 py-3">
							<h3 class="flex-1 text-sm font-semibold text-slate-900">Comments</h3>
							<button class="btn btn-ghost p-1" aria-label="Close comments" onclick={() => (thread = null)}><Icon name="x" class="size-4" /></button>
						</div>
						<div class="flex-1 space-y-2 overflow-y-auto px-3 py-3">
							<MessageItem m={thread.post} chat={current} {me} inThread onchange={put} onplay={play} />
							<hr class="border-slate-100" />
							{#each thread.items as c (c.id)}
								<MessageItem m={c} chat={current} {me} inThread onchange={put} onplay={play} />
							{:else}
								<p class="py-4 text-center text-xs text-slate-400">No comments yet.</p>
							{/each}
						</div>
						{#if current.my_role}<Composer chatId={current.id} parentId={thread.post.id} placeholder="Write a comment…" onsent={put} />{/if}
					</aside>
				{/if}
			</div>
		{:else}
			<div class="m-auto max-w-sm p-8 text-center text-sm text-slate-500"><Icon name="message" class="mx-auto mb-3 size-8 text-slate-300" />Pick a chat, or start a new one with a colleague, a group or a channel.</div>
		{/if}
	</section>
</div>

{#if current}
	<ChatSettings bind:open={settingsOpen} chat={current} {me} onchange={(c) => ((current = c), loadChats())} onleft={() => ((current = null), goto('/messenger', { replace: true }), loadChats())} oncleared={() => current && loadMessages(current.id)} />
	{#if make}<MakeFromChat bind:open={makeOpen} what={make.what} chatId={current.id} message={make.message} />{/if}
{/if}

<Modal bind:open={newOpen} title="New">
	<div class="mb-4 flex rounded-full bg-slate-100 p-0.5 text-sm" role="group">
		{#each [['direct', 'With one person'], ['group', 'Group chat'], ['channel', 'Channel']] as [k, l] (k)}<button class="flex-1 rounded-full px-3 py-1.5 font-medium {newKind === k ? 'bg-white shadow-sm' : 'text-slate-600'}" aria-pressed={newKind === k} onclick={() => (newKind = k as 'direct')}>{l}</button>{/each}
	</div>
	{#if newKind === 'direct'}
		{#if directWith}
			<div class="flex items-center gap-3 rounded-lg border border-slate-200 px-3 py-2" data-direct-with>
				<span class="flex-1 font-medium">{directWith.full_name ?? directWith.name}</span>
				<button type="button" class="text-sm text-slate-500 hover:underline" onclick={() => (directWith = null)}>Change</button>
			</div>
			<div class="mt-4 flex gap-2">
				<button class="btn btn-primary flex-1 justify-center" {@attach (el) => el.focus()} onclick={() => startDirect(false)}><Icon name="message" class="size-4" /> Start chat</button>
				<button class="btn btn-secondary flex-1 justify-center" onclick={() => startDirect(true)}>📹 Video call</button>
			</div>
		{:else}
			<span class="label">Colleague</span>
			<UserPicker placeholder="Type a name…" exclude={me ? [me] : []} onpick={(u) => (directWith = u)} />
			<p class="mt-2 text-xs text-slate-500">Choose someone, then start a chat or a video call. If you already have a chat with them, it opens.</p>
		{/if}
	{:else}
		<form id="grp" class="space-y-3" onsubmit={startGroup}>
			{#if newKind === 'channel'}<p class="text-xs text-slate-500">In a channel, you and the admins you choose post; everyone else reads and comments.</p>{/if}
			<div><label class="label" for="grp-t">Name</label><input id="grp-t" class="input" required maxlength="120" bind:value={form.title} /></div>
			<div><label class="label" for="grp-d">Description</label><input id="grp-d" class="input" maxlength="2000" bind:value={form.description} /></div>
			<div>
				<label class="label" for="grp-a">Members</label>
				<select id="grp-a" class="input" bind:value={form.audience}>
					<option value="manual">People I choose</option>
					{#if departments.length}<option value="department">A department (members follow it)</option>{/if}
					{#if session.can(PERM.SETTINGS_MANAGE)}<option value="everyone">Everyone in the company</option>{/if}
				</select>
			</div>
			{#if form.audience === 'manual'}
				<div>
					<label class="label" for="grp-v">Who finds it</label>
					<select id="grp-v" class="input" bind:value={form.visibility}><option value="private">Only the people added</option><option value="open">Everyone: anyone can join</option></select>
				</div>
				<div><span class="label">People</span><MultiUserPicker bind:value={groupPeople} exclude={me ? [me] : []} /></div>
			{:else if form.audience === 'department'}
				<div>
					<label class="label" for="grp-dep">Department</label>
					<select id="grp-dep" class="input" required bind:value={form.department_id}>
						<option value="" disabled>Choose…</option>
						{#each departments as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
					</select>
					<p class="mt-1 text-xs text-slate-500">Everyone in it and its sub-departments, now and later.</p>
				</div>
			{/if}
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (newOpen = false)}>Cancel</button>
		{#if newKind !== 'direct'}<button class="btn btn-primary" form="grp">Create</button>{/if}
	{/snippet}
</Modal>

<Modal bind:open={findOpen} title="Open chats and channels">
	<input class="input mb-3" type="search" placeholder="Search…" aria-label="Search open chats" bind:value={findQ} oninput={() => (clearTimeout(t), (t = setTimeout(find, 250)))} />
	<ul class="max-h-96 divide-y divide-slate-100 overflow-y-auto">
		{#each found as f (f.id)}
			<li>
				<button class="flex w-full items-center gap-3 py-2.5 text-left hover:bg-slate-50" onclick={() => ((findOpen = false), pick(f.id))} data-found={f.title}>
					<span class="text-lg">{f.kind === 'channel' ? '📣' : '💬'}</span>
					<span class="min-w-0 flex-1"><span class="block truncate font-medium text-slate-900">{f.title}</span>{#if f.description}<span class="block truncate text-xs text-slate-500">{f.description}</span>{/if}</span>
					<span class="text-xs text-slate-400">{f.members} {f.members === 1 ? 'member' : 'members'}</span>
				</button>
			</li>
		{:else}
			<li class="py-8 text-center text-sm text-slate-500">No open chats to join.</li>
		{/each}
	</ul>
</Modal>

<Modal bind:open={folderOpen} title={folderForm.id ? 'Folder' : 'New folder'}>
	<form id="folder-form" class="space-y-3" onsubmit={saveFolder}>
		<div><label class="label" for="fd-name">Name</label><input id="fd-name" class="input" required maxlength="60" bind:value={folderForm.name} /></div>
		<div>
			<span class="label">Chats in it</span>
			<ul class="max-h-64 overflow-y-auto rounded-lg border border-slate-200">
				{#each allChats as c (c.id)}
					<li><label class="flex items-center gap-2 px-3 py-1.5 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" value={c.id} bind:group={folderForm.chat_ids} />{c.title}</label></li>
				{/each}
			</ul>
		</div>
		{#if folderForm.id}<button type="button" class="text-sm text-red-600 hover:underline" onclick={deleteFolder}>Delete folder</button>{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (folderOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="folder-form">Save</button>
	{/snippet}
</Modal>

<Modal open={!!player} title={player?.name ?? ''} size="lg" onclose={() => (player = null)}>
	{#if player}
		<!-- svelte-ignore a11y_media_has_caption -->
		<video class="w-full rounded-lg bg-black" controls autoplay preload="metadata" src={player.url} data-player></video>
	{/if}
</Modal>
