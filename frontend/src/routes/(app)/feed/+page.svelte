<script lang="ts">
	import RequestDialog from '#lib/components/RequestDialog.svelte';
	// News feed: post to everyone or to chosen people and departments; important posts first.
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import FeedPost, { type Post } from '#lib/components/FeedPost.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	let posts = $state<Post[]>([]);
	let important = $state<Post[]>([]);
	let more = $state(false);
	let q = $state('');
	// Moderators: every post, whoever it was for.
	let all = $state(false);
	const moderator = $derived(session.can(PERM.SETTINGS_MANAGE) || session.can(PERM.FEED_MODERATE));
	let fresh = $state(0);

	async function load() {
		const r = await api.get<{ items: Post[]; more: boolean }>('/feed' + query({ q: q.trim() || undefined, all: all || undefined }));
		posts = r.items;
		more = r.more;
		fresh = 0;
		important = (await api.get<{ items: Post[] }>('/feed/important')).items;
	}
	async function older() {
		const r = await api.get<{ items: Post[]; more: boolean }>('/feed' + query({ q: q.trim() || undefined, all: all || undefined, before: posts.at(-1)?.created_at }));
		posts = [...posts, ...r.items];
		more = r.more;
	}
	onMount(() => {
		load();
		const off = [
			realtime.on('feed.post', () => fresh++),
			realtime.on('feed.changed', async (d: { post_id: string }) => {
				const i = posts.findIndex((p) => p.id === d.post_id);
				if (i >= 0) posts[i] = await api.get<Post>(`/feed/${d.post_id}`);
			})
		];
		return () => off.forEach((f) => f());
	});
	let timer: ReturnType<typeof setTimeout> | undefined;
	const search = () => (clearTimeout(timer), (timer = setTimeout(load, 300)));
	const highlight = $derived(page.url.searchParams.get('post'));

	// Writing a post.
	let body = $state('');
	let toAll = $state(true);
	let people = $state<{ id: string; name: string; avatar_file_id: string | null }[]>([]);
	let departments = $state<{ id: string; name: string }[]>([]);
	let chosenDepts = $state<string[]>([]);
	let isImportant = $state(false);
	let withPoll = $state(false);
	// Appreciation: a badge for colleagues.
	let withBadge = $state(false);
	let badge = $state('thanks');
	let badgePeople = $state<{ id: string; name: string; avatar_file_id: string | null }[]>([]);
	const BADGES: [string, string][] = [['thanks', '🙏 Thank you'], ['team_player', '🤝 Team player'], ['great_idea', '💡 Great idea'], ['problem_solver', '🧩 Problem solver'], ['leader', '⭐ Leader'], ['above_beyond', '🚀 Above and beyond']];
	let poll = $state({ question: '', options: ['', ''] });
	let files = $state<{ id: string; name: string }[]>([]);
	let busy = $state(false);
	let requestOpen = $state(false);
	onMount(() => {
		api.get<{ id: string; name: string }[]>('/departments').then((d) => (departments = d)).catch(() => {});
	});
	async function upload(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		for (const f of Array.from(input.files ?? [])) {
			const fd = new FormData();
			fd.append('file', f);
			try {
				const up = await api.post<{ id: string }>('/files', fd);
				files = [...files, { id: up.id, name: f.name }];
			} catch (err) {
				toasts.error(err);
			}
		}
		input.value = '';
	}
	async function publish(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const p = await api.post<Post>('/feed', {
				body,
				to_all: toAll,
				users: toAll ? [] : people.map((x) => x.id),
				departments: toAll ? [] : chosenDepts,
				important: isImportant,
				file_ids: files.map((f) => f.id),
				poll: withPoll ? { question: poll.question, options: poll.options } : null,
				badge: withBadge ? badge : null,
				badge_user_ids: withBadge ? badgePeople.map((p) => p.id) : []
			});
			withBadge = false;
			badgePeople = [];
			posts = [p, ...posts.filter((x) => x.id !== p.id)];
			body = '';
			files = [];
			isImportant = false;
			withPoll = false;
			poll = { question: '', options: ['', ''] };
			toAll = true;
			people = [];
			chosenDepts = [];
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>News feed · NERP</title></svelte:head>

<div class="mx-auto max-w-2xl space-y-4">
	<div class="flex items-center gap-3">
		<h1 class="flex-1 text-2xl font-semibold text-slate-900">News feed</h1>
		<input class="input w-56" type="search" placeholder="Search posts" aria-label="Search posts" bind:value={q} oninput={search} />
		{#if moderator}<label class="flex items-center gap-1.5 text-sm text-slate-600" title="Also posts for a few people, and workgroups you're not in"><input type="checkbox" class="rounded border-slate-300" bind:checked={all} onchange={search} /> Every post</label>{/if}
	</div>

	<form class="card space-y-3 p-5" onsubmit={publish} aria-label="New post">
		<textarea class="input" rows="3" maxlength="20000" placeholder="Share news with your colleagues…" aria-label="Post" bind:value={body}></textarea>
		{#if withPoll}
			<div class="space-y-2 rounded-xl bg-slate-50 p-3">
				<input class="input" required maxlength="300" placeholder="Question" aria-label="Poll question" bind:value={poll.question} />
				{#each poll.options as _, i (i)}<input class="input" maxlength="200" placeholder="Answer {i + 1}" aria-label="Answer {i + 1}" bind:value={poll.options[i]} />{/each}
				{#if poll.options.length < 10}<button type="button" class="text-sm text-brand-600 hover:underline" onclick={() => poll.options.push('')}>+ Answer</button>{/if}
			</div>
		{/if}
		{#if withBadge}
			<div class="space-y-2 rounded-xl bg-amber-50 p-3" aria-label="Badge">
				<div class="flex flex-wrap gap-1.5" role="group" aria-label="Which badge">{#each BADGES as [k, l] (k)}<button type="button" class="rounded-full border px-3 py-1 text-sm {badge === k ? 'border-amber-500 bg-white font-medium' : 'border-amber-200 text-amber-900'}" aria-pressed={badge === k} onclick={() => (badge = k)}>{l}</button>{/each}</div>
				<MultiUserPicker bind:value={badgePeople} placeholder="For whom…" />
			</div>
		{/if}
		{#if !toAll}
			<div class="space-y-2 rounded-xl bg-slate-50 p-3">
				<MultiUserPicker bind:value={people} placeholder="People…" />
				{#if departments.length}
					<div class="flex flex-wrap gap-1.5" role="group" aria-label="Departments">
						{#each departments as d (d.id)}<button type="button" class="rounded-full border px-3 py-1 text-sm {chosenDepts.includes(d.id) ? 'border-brand-500 bg-brand-50 text-brand-700' : 'border-slate-200 text-slate-600'}" aria-pressed={chosenDepts.includes(d.id)} onclick={() => (chosenDepts = chosenDepts.includes(d.id) ? chosenDepts.filter((x) => x !== d.id) : [...chosenDepts, d.id])}>{d.name}</button>{/each}
					</div>
				{/if}
			</div>
		{/if}
		{#if files.length}<div class="flex flex-wrap gap-2">{#each files as f, i (f.id)}<span class="flex items-center gap-1 rounded-lg bg-slate-100 px-2 py-1 text-xs">{f.name}<button type="button" aria-label="Remove {f.name}" onclick={() => files.splice(i, 1)}><Icon name="x" class="size-3" /></button></span>{/each}</div>{/if}
		<div class="flex flex-wrap items-center gap-3 text-sm">
			<select class="input w-40" aria-label="Who sees it" bind:value={toAll}><option value={true}>Everyone</option><option value={false}>Chosen people…</option></select>
			<label class="btn btn-ghost cursor-pointer px-2" aria-label="Attach files"><Icon name="paperclip" class="size-4" /><input type="file" multiple class="sr-only" onchange={upload} /></label>
			<button type="button" class="btn btn-ghost px-2 {withPoll ? 'text-brand-700' : ''}" aria-pressed={withPoll} onclick={() => (withPoll = !withPoll)}>Poll</button>
			<button type="button" class="btn btn-ghost px-2 {withBadge ? 'text-brand-700' : ''}" aria-pressed={withBadge} onclick={() => (withBadge = !withBadge)}>Give a badge</button>
			<button type="button" class="btn btn-ghost px-2" onclick={() => (requestOpen = true)} data-new-request>Request…</button>
			<label class="flex items-center gap-2 text-slate-600"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={isImportant} /> Important</label>
			<button class="btn btn-primary ml-auto" disabled={busy || !body.trim()}>Post</button>
		</div>
	</form>

	{#if important.length}
		<section class="space-y-3" aria-label="Important">
			<h2 class="text-sm font-medium text-amber-800">Important, not read yet</h2>
			{#each important as p, i (p.id)}<FeedPost bind:post={important[i]} onremoved={load} />{/each}
		</section>
	{/if}

	{#if fresh}<button class="btn btn-secondary w-full justify-center" onclick={load}>{fresh} new post{fresh === 1 ? '' : 's'}</button>{/if}

	{#each posts as p, i (p.id)}
		{#if !important.some((x) => x.id === p.id)}
			<div class={highlight === p.id ? 'rounded-2xl ring-2 ring-brand-400' : ''}><FeedPost bind:post={posts[i]} onremoved={() => (posts = posts.filter((x) => x.id !== p.id))} /></div>
		{/if}
	{:else}
		<div class="card p-10 text-center text-slate-500">{q ? 'No posts match.' : 'Nothing posted yet.'}</div>
	{/each}
	{#if more}<button class="btn btn-secondary w-full justify-center" onclick={older}>Older posts</button>{/if}
</div>

<RequestDialog bind:open={requestOpen} onposted={(p) => (posts = [p, ...posts])} />
