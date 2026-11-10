<script lang="ts">
	// A workgroup's news feed: posts to the group's members, with files.
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import FeedPost, { type Post } from '#lib/components/FeedPost.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { projectId, canPost, owner }: { projectId: string; canPost: boolean; owner: boolean } = $props();

	let posts = $state<Post[] | null>(null);
	let more = $state(false);
	let body = $state('');
	let files = $state<{ id: string; name: string }[]>([]);
	let busy = $state(false);

	async function load() {
		const r = await api.get<{ items: Post[]; more: boolean }>('/feed' + query({ project_id: projectId }));
		posts = r.items;
		more = r.more;
	}
	async function older() {
		const r = await api.get<{ items: Post[]; more: boolean }>('/feed' + query({ project_id: projectId, before: posts?.at(-1)?.created_at }));
		posts = [...(posts ?? []), ...r.items];
		more = r.more;
	}
	onMount(() => {
		load();
		return realtime.on('feed.post', load);
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
			const p = await api.post<Post>('/feed', { body, project_id: projectId, file_ids: files.map((f) => f.id) });
			posts = [p, ...(posts ?? []).filter((x) => x.id !== p.id)];
			body = '';
			files = [];
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

<div class="max-w-2xl space-y-4">
	{#if canPost}
		<form class="card space-y-3 p-5" onsubmit={publish} aria-label="New group post">
			<textarea class="input" rows="3" maxlength="20000" placeholder="Share something with the group…" aria-label="Post" bind:value={body}></textarea>
			{#if files.length}<div class="flex flex-wrap gap-2">{#each files as f, i (f.id)}<span class="flex items-center gap-1 rounded-lg bg-slate-100 px-2 py-1 text-xs">{f.name}<button type="button" aria-label="Remove {f.name}" onclick={() => files.splice(i, 1)}><Icon name="x" class="size-3" /></button></span>{/each}</div>{/if}
			<div class="flex items-center gap-3 text-sm">
				<label class="btn btn-ghost cursor-pointer px-2" aria-label="Attach files"><Icon name="paperclip" class="size-4" /><input type="file" multiple class="sr-only" onchange={upload} /></label>
				<button class="btn btn-primary ml-auto" disabled={busy || !body.trim()}>Post</button>
			</div>
		</form>
	{/if}
	{#if posts}
		{#each posts as p, i (p.id)}
			<FeedPost bind:post={posts[i]} showGroup={false} groupOwner={owner} onremoved={() => (posts = (posts ?? []).filter((x) => x.id !== p.id))} />
		{:else}
			<div class="card p-10 text-center text-slate-500">Nothing posted in this group yet.</div>
		{/each}
		{#if more}<button class="btn btn-secondary w-full justify-center" onclick={older}>Older posts</button>{/if}
	{/if}
</div>
