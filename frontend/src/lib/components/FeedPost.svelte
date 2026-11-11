<script module lang="ts">
	export type FeedFile = { id: string; name: string; size: number; mime: string };
	export type Post = {
		id: string;
		author_id: string | null;
		author_name: string | null;
		author_avatar: string | null;
		body: string;
		files: FeedFile[];
		to_all: boolean;
		important: boolean;
		poll: { question: string; options: string[] } | null;
		created_at: string;
		edited_at: string | null;
		likes: number;
		liked: boolean;
		reactions: Record<string, number>;
		my_reaction: string | null;
		badge: string | null;
		badge_people: { id: string; name: string; avatar_file_id: string | null }[];
		/** A board made for the post. */
		board?: { id: string; title: string; level: 'view' | 'edit' } | null;
		comments: number;
		read_by_me: boolean;
		reads: number;
		votes: Record<string, number>;
		my_vote: number | null;
		to: string[];
		readers?: { id: string; name: string; read_at: string | null }[];
		project_id: string | null;
		project_name: string | null;
		/** A request started from the feed (a list item and its process). */
		request?: { item_id: string; list_id: string; list_name: string; title: string; number: number; status: string | null; approvals: { id: string; title: string }[] } | null;
	};
</script>

<script lang="ts">
	// One post in the news feed: text, files, poll, likes, comments, "I've read it".
	import { api, downloadFile } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import VideoPlayer from '#lib/components/VideoPlayer.svelte';
	import { formatBytes, formatRelative } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	let {
		post = $bindable(),
		onremoved,
		showGroup = true,
		groupOwner = false
	}: { post: Post; onremoved?: () => void; showGroup?: boolean; groupOwner?: boolean } = $props();
	type Comment = {
		id: string;
		author_id: string | null;
		author_name: string | null;
		author_avatar: string | null;
		body: string;
		created_at: string;
	};
	let comments = $state<Comment[] | null>(null);
	let draft = $state('');
	let editing = $state(false);
	let editText = $state('');
	let showReaders = $state(false);
	const mine = $derived(post.author_id === session.user?.id);
	const moderator = $derived(session.can(PERM.SETTINGS_MANAGE) || session.can(PERM.FEED_MODERATE));
	const totalVotes = $derived(Object.values(post.votes).reduce((a, b) => a + b, 0));

	async function act(fn: () => Promise<Post>) {
		try {
			post = await fn();
		} catch (e) {
			toasts.error(e);
		}
	}
	const REACTIONS: [string, string][] = [
		['like', '👍'],
		['love', '❤️'],
		['laugh', '😄'],
		['wow', '😮'],
		['sad', '😢'],
		['clap', '👏']
	];
	const BADGE: Record<string, [string, string]> = {
		thanks: ['🙏', 'Thank you'],
		team_player: ['🤝', 'Team player'],
		great_idea: ['💡', 'Great idea'],
		problem_solver: ['🧩', 'Problem solver'],
		leader: ['⭐', 'Leader'],
		above_beyond: ['🚀', 'Above and beyond']
	};
	let picking = $state(false);
	const react = (r: string) => {
		picking = false;
		return act(() =>
			post.my_reaction === r
				? api.del<Post>(`/feed/${post.id}/like`)
				: api.post<Post>(`/feed/${post.id}/like`, { reaction: r })
		);
	};
	const confirmRead = () => act(() => api.post<Post>(`/feed/${post.id}/read`, {}));
	const voteFor = (i: number) => act(() => api.post<Post>(`/feed/${post.id}/vote`, { option: i }));
	async function openComments() {
		comments = (await api.get<{ items: Comment[] }>(`/feed/${post.id}/comments`)).items;
	}
	async function addComment(e: SubmitEvent) {
		e.preventDefault();
		try {
			comments = (
				await api.post<{ items: Comment[] }>(`/feed/${post.id}/comments`, { body: draft })
			).items;
			post.comments = comments.length;
			draft = '';
		} catch (err) {
			toasts.error(err);
		}
	}
	async function editComment(c: Comment) {
		const body = prompt('Change the comment', c.body);
		if (body === null || !body.trim() || body === c.body) return;
		try {
			comments = (await api.patch<{ items: Comment[] }>(`/feed/comments/${c.id}`, { body })).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function removeComment(c: Comment) {
		if (!confirm('Delete this comment?')) return;
		comments = (await api.del<{ items: Comment[] }>(`/feed/comments/${c.id}`)).items;
		post.comments = comments.length;
	}
	async function save() {
		await act(() => api.patch<Post>(`/feed/${post.id}`, { body: editText }));
		editing = false;
	}
	async function remove() {
		if (!confirm('Delete this post?')) return;
		await api.del(`/feed/${post.id}`);
		onremoved?.();
	}
	async function readers() {
		showReaders = !showReaders;
		if (showReaders) post = await api.get<Post>(`/feed/${post.id}`);
	}
	const REQUEST_STATUS: Record<string, [string, string]> = {
		running: ['In progress', 'bg-sky-50 text-sky-800'],
		waiting: ['Waiting', 'bg-amber-50 text-amber-800'],
		completed: ['Done', 'bg-emerald-50 text-emerald-800'],
		rejected: ['Rejected', 'bg-red-50 text-red-700'],
		cancelled: ['Cancelled', 'bg-slate-100 text-slate-600'],
		failed: ['Stopped', 'bg-red-50 text-red-700']
	};
	async function decide(approval: string, approve: boolean) {
		const comment = approve ? null : prompt('Why? (optional)');
		try {
			await api.patch(`/approvals/${approval}`, { approve, comment });
			post = await api.get<Post>(`/feed/${post.id}`);
			toasts.success(approve ? 'Approved' : 'Rejected');
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<article
	class="card p-5 {post.important && !post.read_by_me && !mine ? 'ring-2 ring-amber-300' : ''}"
	data-post={post.body.slice(0, 60)}
>
	<header class="flex items-start gap-3">
		<Avatar name={post.author_name ?? '?'} fileId={post.author_avatar} size="md" />
		<div class="min-w-0 flex-1">
			<p class="font-medium text-slate-900">
				{post.author_name ?? 'Former employee'}{#if post.important}<span
						class="badge ml-2 bg-amber-50 text-amber-800">Important</span
					>{/if}
			</p>
			<p class="text-xs text-slate-500">
				{formatRelative(post.created_at)}{post.edited_at ? ' · edited' : ''} · {#if post.project_id}{#if showGroup}In
						<a class="text-brand-700 hover:underline" href="/projects/{post.project_id}"
							>{post.project_name}</a
						>{:else}The group{/if}{:else}{post.to_all
						? 'Everyone'
						: `To ${post.to.join(', ')}`}{/if}
			</p>
		</div>
		{#if (mine || moderator) && !editing}<button
				class="btn btn-ghost p-1.5"
				aria-label="Edit post"
				onclick={() => ((editing = true), (editText = post.body))}
				><Icon name="edit" class="size-4" /></button
			>{/if}
		{#if mine || moderator || groupOwner}<button
				class="btn btn-ghost p-1.5 hover:text-red-600"
				aria-label="Delete post"
				onclick={remove}><Icon name="trash" class="size-4" /></button
			>{/if}
	</header>

	{#if editing}
		<div class="mt-3 space-y-2">
			<textarea class="input" rows="4" bind:value={editText} aria-label="Edit post"></textarea>
			<div class="flex gap-2">
				<button class="btn btn-primary" onclick={save}>Save</button><button
					class="btn btn-ghost"
					onclick={() => (editing = false)}>Cancel</button
				>
			</div>
		</div>
	{:else}
		<p class="mt-3 break-words whitespace-pre-line text-slate-800">{post.body}</p>
	{/if}

	{#each post.files.filter((f) => (f.mime ?? '').startsWith('video/')) as v (v.id)}
		<VideoPlayer fileId={v.id} name={v.name} class="mt-3" />
	{/each}
	{#if post.files.length}
		<div class="mt-3 flex flex-wrap gap-2">
			{#each post.files as f (f.id)}<button
					class="flex items-center gap-2 rounded-lg bg-slate-50 px-3 py-1.5 text-sm hover:bg-slate-100"
					onclick={() => downloadFile(f.id, f.name).catch(toasts.error)}
					><Icon name="paperclip" class="size-4" />{f.name}
					<span class="text-xs text-slate-400">{formatBytes(f.size)}</span></button
				>{/each}
		</div>
	{/if}

	{#if post.request}
		{@const rq = post.request}
		<div class="mt-4 space-y-2 rounded-xl border border-slate-200 p-4" data-request>
			<div class="flex flex-wrap items-center gap-2">
				<span class="text-xs font-medium tracking-wide text-slate-500 uppercase">{rq.list_name}</span>
				<a class="font-medium text-slate-900 hover:underline" href="/lists/{rq.list_id}?item={rq.item_id}">#{rq.number} {rq.title}</a>
				{#if rq.status}<span class="badge {REQUEST_STATUS[rq.status]?.[1] ?? 'bg-slate-100 text-slate-600'}" data-request-status={rq.status}>{REQUEST_STATUS[rq.status]?.[0] ?? rq.status}</span>{/if}
			</div>
			{#each rq.approvals as a (a.id)}
				<div class="flex flex-wrap items-center gap-2 rounded-lg bg-amber-50 px-3 py-2 text-sm" data-my-approval>
					<span class="flex-1 text-amber-900">{a.title}</span>
					<button class="btn btn-primary px-3 py-1 text-xs" onclick={() => decide(a.id, true)}>Approve</button>
					<button class="btn btn-secondary px-3 py-1 text-xs" onclick={() => decide(a.id, false)}>Reject</button>
				</div>
			{/each}
		</div>
	{/if}

	{#if post.poll}
		<div class="mt-4 space-y-2 rounded-xl border border-slate-200 p-4" data-poll>
			<p class="font-medium text-slate-900">{post.poll.question}</p>
			{#each post.poll.options as o, i (i)}
				{@const n = post.votes[String(i)] ?? 0}
				<button
					class="relative block w-full overflow-hidden rounded-lg border px-3 py-2 text-left text-sm {post.my_vote ===
					i
						? 'border-brand-500'
						: 'border-slate-200 hover:border-slate-400'}"
					aria-pressed={post.my_vote === i}
					onclick={() => voteFor(i)}
				>
					{#if post.my_vote !== null}<span
							class="absolute inset-y-0 left-0 bg-brand-50"
							style="width:{totalVotes ? (n / totalVotes) * 100 : 0}%"
						></span>{/if}
					<span class="relative flex justify-between"
						><span>{o}</span>{#if post.my_vote !== null}<span class="text-slate-500 tabular-nums"
								>{n}</span
							>{/if}</span
					>
				</button>
			{/each}
			<p class="text-xs text-slate-500">
				{totalVotes} vote{totalVotes === 1 ? '' : 's'}{post.my_vote === null
					? ' · vote to see the results'
					: ''}
			</p>
		</div>
	{/if}

	{#if post.important && !mine}
		{#if post.read_by_me}<p class="mt-3 text-sm text-emerald-700">✓ You've read this</p>
		{:else}<button class="btn btn-primary mt-3" onclick={confirmRead}>I've read it</button>{/if}
	{/if}
	{#if post.important && mine}
		<button class="mt-3 text-sm text-brand-600 hover:underline" onclick={readers}
			>Read by {post.reads}</button
		>
		{#if showReaders && post.readers}
			<ul class="mt-2 space-y-0.5 text-sm" data-readers>
				{#each post.readers as r (r.id)}<li class={r.read_at ? 'text-slate-800' : 'text-slate-400'}>
						{r.name}
						{r.read_at ? `· ${formatRelative(r.read_at)}` : '· not yet'}
					</li>{/each}
			</ul>
		{/if}
	{/if}

	{#if post.badge && BADGE[post.badge]}
		<div
			class="mt-3 flex items-center gap-3 rounded-xl bg-amber-50 px-4 py-3"
			data-badge={post.badge}
		>
			<span class="text-3xl">{BADGE[post.badge][0]}</span>
			<div>
				<p class="font-medium text-amber-900">{BADGE[post.badge][1]}</p>
				<p class="text-sm text-amber-800">for {post.badge_people.map((p) => p.name).join(', ')}</p>
			</div>
		</div>
	{/if}
	{#if post.board}
		<a href="/boards/{post.board.id}" class="mt-3 flex items-center gap-3 rounded-xl border border-slate-200 px-4 py-3 hover:border-brand-400" data-post-board-card>
			<Icon name="board" class="size-5 text-brand-600" />
			<span class="flex-1"><span class="block font-medium text-slate-900">{post.board.title}</span><span class="text-xs text-slate-500">{post.board.level === 'edit' ? 'A board to draw on together' : 'A board to look at'}</span></span>
			<span class="text-sm text-brand-700">Open</span>
		</a>
	{/if}
	<footer class="mt-4 flex gap-4 border-t border-slate-100 pt-3 text-sm">
		<div class="relative flex items-center gap-2">
			<button
				class="flex items-center gap-1.5 {post.my_reaction
					? 'font-medium text-brand-700'
					: 'text-slate-500 hover:text-slate-800'}"
				aria-pressed={!!post.my_reaction}
				onclick={() => react(post.my_reaction ?? 'like')}
				oncontextmenu={(e) => {
					e.preventDefault();
					picking = !picking;
				}}
				data-like
				>{REACTIONS.find(([k]) => k === post.my_reaction)?.[1] ?? '👍'}
				{post.my_reaction ? 'Reacted' : 'Like'}</button
			>
			<button
				class="text-slate-400 hover:text-slate-700"
				aria-label="More reactions"
				aria-expanded={picking}
				onclick={() => (picking = !picking)}>▾</button
			>
			{#if picking}
				<div
					class="absolute bottom-7 left-0 z-10 flex gap-1 rounded-full border border-slate-200 bg-white px-2 py-1 shadow-lg"
					role="group"
					aria-label="Reactions"
				>
					{#each REACTIONS as [k, e] (k)}<button
							class="rounded-full px-1 text-lg hover:scale-125 {post.my_reaction === k
								? 'bg-brand-50'
								: ''}"
							aria-label={k}
							onclick={() => react(k)}>{e}</button
						>{/each}
				</div>
			{/if}
			{#if post.likes}<span class="text-slate-500" data-reactions
					>{REACTIONS.filter(([k]) => post.reactions?.[k])
						.map(([k, e]) => `${e} ${post.reactions[k]}`)
						.join('  ')}</span
				>{/if}
		</div>
		<button
			class="text-slate-500 hover:text-slate-800"
			onclick={() => (comments ? (comments = null) : openComments())}
			>{post.comments
				? `${post.comments} comment${post.comments === 1 ? '' : 's'}`
				: 'Comment'}</button
		>
	</footer>

	{#if comments}
		<div class="mt-3 space-y-3">
			{#each comments as c (c.id)}
				<div class="flex gap-2" data-comment={c.body}>
					<Avatar name={c.author_name ?? '?'} fileId={c.author_avatar} size="sm" />
					<div class="min-w-0 flex-1 rounded-xl bg-slate-50 px-3 py-2 text-sm">
						<p class="text-xs">
							<span class="font-medium text-slate-800">{c.author_name ?? 'Former employee'}</span>
							<span class="text-slate-400">{formatRelative(c.created_at)}</span>
							{#if c.author_id === session.user?.id || moderator}<button
									class="float-right text-slate-300 hover:text-red-600"
									aria-label="Delete comment"
									onclick={() => removeComment(c)}><Icon name="x" class="size-3.5" /></button
								><button
									class="float-right mr-1 text-slate-300 hover:text-brand-700"
									aria-label="Edit comment"
									onclick={() => editComment(c)}><Icon name="edit" class="size-3.5" /></button
								>{/if}
						</p>
						<p class="break-words whitespace-pre-line text-slate-700">{c.body}</p>
					</div>
				</div>
			{/each}
			<form class="flex gap-2" onsubmit={addComment}>
				<input
					class="input flex-1"
					maxlength="5000"
					placeholder="Write a comment…"
					aria-label="Write a comment"
					bind:value={draft}
				/><button class="btn btn-secondary" disabled={!draft.trim()}>Send</button>
			</form>
		</div>
	{/if}
</article>
