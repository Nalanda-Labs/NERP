<script lang="ts">
	import { office } from '#lib/office.svelte.ts';
	// One message: text and files, voice and video notes, stickers, polls, reactions, the
	// comment count of a channel post, and the actions on it.
	import { api, downloadFile } from '#lib/api.ts';
	import { formatBytes } from '#lib/format.ts';
	import { REACTIONS, STICKERS, type ChatDetail, type Msg, type MsgFile } from '#lib/messenger.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Avatar from '../Avatar.svelte';
	import Icon from '../Icon.svelte';
	import MediaNote from './MediaNote.svelte';

	let {
		m,
		chat,
		me,
		first = true,
		seen = [],
		inThread = false,
		onchange,
		oncomments,
		onplay,
		onmake
	}: {
		m: Msg;
		chat: ChatDetail;
		me: string | undefined;
		/** First of a run by the same person: show their avatar and name. */
		first?: boolean;
		/** Who has read it (my latest message). */
		seen?: string[];
		inThread?: boolean;
		onchange: (m: Msg) => void;
		oncomments?: (m: Msg) => void;
		onplay?: (f: MsgFile) => void;
		onmake?: (what: 'task' | 'event', m: Msg) => void;
	} = $props();

	const mine = $derived(m.user_id === me);
	const member = $derived(!!chat.my_role);
	const channelPost = $derived(chat.kind === 'channel' && !m.parent_id);
	let editing = $state(false);
	let editText = $state('');
	let picking = $state(false);
	const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

	async function run<T extends Msg>(p: Promise<T>) {
		try {
			onchange(await p);
		} catch (e) {
			toasts.error(e);
		}
	}
	const react = (emoji: string) => ((picking = false), run(api.post<Msg>(`/im/messages/${m.id}/react`, { emoji })));
	async function vote(i: number) {
		const p = m.poll!;
		const next = p.multiple ? (p.my_votes.includes(i) ? p.my_votes.filter((x) => x !== i) : [...p.my_votes, i]) : p.my_votes.includes(i) ? [] : [i];
		await run(api.post<Msg>(`/im/messages/${m.id}/vote`, { options: next }));
	}
	async function save() {
		await run(api.patch<Msg>(`/im/messages/${m.id}`, { body: editText }));
		editing = false;
	}
	async function del() {
		if (!confirm('Delete this message for everyone?')) return;
		await run(api.del<Msg>(`/im/messages/${m.id}`));
	}
	const canDelete = $derived(!m.deleted && (mine || chat.can.manage));
	const bubble = $derived(m.deleted ? 'bg-slate-50 text-slate-400 italic' : m.kind === 'sticker' ? '' : mine && !channelPost ? 'bg-brand-600 text-white' : 'bg-slate-100 text-slate-800');
</script>

{#if !m.user_id}
	<p class="py-1 text-center text-xs text-slate-400" data-system>{m.body}</p>
{:else}
	<div class="group flex items-end gap-2 {mine && !channelPost && !inThread ? 'flex-row-reverse' : ''}" data-msg={m.body} data-msg-id={m.id} data-kind={m.kind}>
		<div class="w-8 shrink-0">{#if first && (!mine || channelPost || inThread)}<Avatar name={m.user_name ?? ''} fileId={m.avatar_file_id} size="sm" />{/if}</div>
		<div class="min-w-0 {channelPost ? 'max-w-[85%] flex-1' : 'max-w-[70%]'}">
			{#if first && (!mine || channelPost || inThread) && chat.kind !== 'direct'}<p class="mb-0.5 text-xs text-slate-500">{m.user_name}</p>{/if}
			{#if editing}
				<div class="flex gap-2">
					<input class="input" bind:value={editText} aria-label="Edit message" onkeydown={(e) => e.key === 'Enter' && save()} />
					<button class="btn btn-primary px-3" onclick={save}>Save</button>
					<button class="btn btn-ghost px-2" onclick={() => (editing = false)}>Cancel</button>
				</div>
			{:else}
				<div class="rounded-2xl px-3.5 py-2 text-sm {bubble} {channelPost && !m.deleted ? 'border border-slate-200 bg-white text-slate-800' : ''}">
					{#if m.deleted}Message deleted{:else if m.kind === 'sticker'}
						<span class="block text-6xl leading-none" role="img" aria-label="Sticker: {STICKERS[m.body]?.label ?? m.body}" data-sticker={m.body}>{STICKERS[m.body]?.emoji ?? '❓'}</span>
					{:else if m.kind === 'poll' && m.poll}
						{@const p = m.poll}
						<div class="min-w-60" data-poll={p.question}>
							<p class="font-semibold">📊 {p.question}</p>
							<p class="mb-2 text-xs opacity-70">{p.anonymous ? 'Anonymous · ' : ''}{p.multiple ? 'Several answers · ' : ''}{p.voters} {p.voters === 1 ? 'vote' : 'votes'}{p.closed ? ' · Closed' : ''}</p>
							<ul class="space-y-1.5">
								{#each p.options as o, i (i)}
									{@const share = p.voters ? Math.round((100 * o.votes) / p.voters) : 0}
									<li>
										<button
											class="relative w-full overflow-hidden rounded-lg border px-2.5 py-1.5 text-left {p.my_votes.includes(i) ? 'border-brand-500' : 'border-slate-300/60'} disabled:cursor-default"
											disabled={p.closed || !member}
											onclick={() => vote(i)}
											title={o.voters.join(', ')}
											data-option={o.text}
										>
											<span class="absolute inset-y-0 left-0 bg-brand-500/15" style="width: {share}%"></span>
											<span class="relative flex gap-2"><span class="flex-1">{p.my_votes.includes(i) ? '✓ ' : ''}{o.text}</span><span class="text-xs opacity-70">{o.votes} · {share}%</span></span>
										</button>
									</li>
								{/each}
							</ul>
							{#if !p.closed && (mine || chat.can.manage)}
								<button class="mt-2 text-xs underline opacity-70" onclick={() => run(api.post<Msg>(`/im/messages/${m.id}/close-poll`))}>Close the poll</button>
							{/if}
						</div>
					{:else if (m.kind === 'voice' || m.kind === 'video') && m.files[0]}
						<MediaNote file={m.files[0]} kind={m.kind} />
					{:else}
						{#if m.body}<p class="break-words whitespace-pre-line">{m.body}</p>{/if}
						{#each m.files as f (f.id)}
							<span class="mt-1 flex items-center gap-1">
								<button class="flex items-center gap-2 rounded-lg px-2 py-1 text-left text-xs {mine && !channelPost ? 'bg-white/15' : 'bg-white'}" onclick={() => downloadFile(f.id, f.name).catch(toasts.error)}><Icon name="paperclip" class="size-3.5" />{f.name} <span class="opacity-70">{formatBytes(f.size)}</span></button>{#if office.opens(f.name)}<a class="ml-1 text-xs font-medium underline opacity-80 hover:opacity-100" href="/files/edit/{f.id}" data-edit-attachment={f.name}>Edit</a>{/if}
								{#if f.mime.startsWith('video/') || f.mime.startsWith('audio/')}<button class="rounded-lg px-2 py-1 text-xs {mine && !channelPost ? 'bg-white/15' : 'bg-white'}" aria-label="Play {f.name}" onclick={() => onplay?.(f)}>▶ Play</button>{/if}
							</span>
						{/each}
					{/if}
					{#if m.kind !== 'sticker' || m.deleted}<p class="mt-0.5 text-right text-[10px] opacity-60">{m.edited_at && !m.deleted ? 'edited · ' : ''}{time(m.created_at)}</p>{/if}
				</div>
				{#if m.reactions.length}
					<div class="mt-1 flex flex-wrap gap-1 {mine && !channelPost && !inThread ? 'justify-end' : ''}">
						{#each m.reactions as r (r.emoji)}
							<button class="rounded-full border px-1.5 py-0.5 text-xs {r.mine ? 'border-brand-400 bg-brand-50' : 'border-slate-200 bg-white'}" title={r.names.join(', ')} disabled={!member} onclick={() => react(r.emoji)} data-reaction={r.emoji}>{r.emoji} {r.count}</button>
						{/each}
					</div>
				{/if}
				{#if channelPost && !m.deleted && !inThread}
					<button class="mt-1 text-xs text-brand-600 hover:underline" onclick={() => oncomments?.(m)} data-comments>{m.comment_count ? `${m.comment_count} ${m.comment_count === 1 ? 'comment' : 'comments'}` : 'Comment'}</button>
				{/if}
				{#if seen.length}<p class="mt-0.5 text-right text-[10px] text-slate-400" data-seen>Seen{chat.kind !== 'direct' ? ` by ${seen.join(', ')}` : ''}</p>{/if}
			{/if}
		</div>
		{#if !m.deleted && !editing && member}
			<div class="relative hidden shrink-0 items-center gap-1 self-center group-hover:flex group-focus-within:flex">
				<button class="text-slate-400 hover:text-slate-700" aria-label="React" onclick={() => (picking = !picking)}>☺</button>
				{#if picking}
					<div class="absolute bottom-6 z-10 flex gap-1 rounded-full border border-slate-200 bg-white px-2 py-1 shadow-lg {mine ? 'right-0' : 'left-0'}" role="menu" aria-label="Reactions">
						{#each REACTIONS as e (e)}<button class="text-lg hover:scale-125" role="menuitem" onclick={() => react(e)}>{e}</button>{/each}
					</div>
				{/if}
				{#if onmake && m.kind === 'text'}
					<button class="text-slate-400 hover:text-slate-700" aria-label="Make a task" title="Make a task" onclick={() => onmake('task', m)}><Icon name="checkSquare" class="size-3.5" /></button>
					<button class="text-slate-400 hover:text-slate-700" aria-label="Plan an event" title="Plan an event" onclick={() => onmake('event', m)}><Icon name="calendar" class="size-3.5" /></button>
				{/if}
				{#if mine && m.kind === 'text'}<button class="text-slate-400 hover:text-slate-700" aria-label="Edit" onclick={() => ((editing = true), (editText = m.body))}><Icon name="edit" class="size-3.5" /></button>{/if}
				{#if canDelete}<button class="text-slate-400 hover:text-red-600" aria-label="Delete" onclick={del}><Icon name="trash" class="size-3.5" /></button>{/if}
			</div>
		{/if}
	</div>
{/if}
