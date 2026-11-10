<script lang="ts">
	// A lesson: read it (text, video, files), try its self-check questions, mark it done and go
	// on. People who manage training edit it here (?edit=1).
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, downloadFile } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { CourseView, Lesson } from '#lib/training.ts';

	const id = $derived(page.params.id);
	let l = $state<Lesson | null>(null);
	let error = $state('');
	let editing = $state(false);
	async function load() {
		error = '';
		try {
			l = await api.get<Lesson>(`/training/lessons/${id}`);
			editing = l.can_manage && page.url.searchParams.get('edit') === '1';
			checked = {};
			chosen = {};
			if (editing) startEdit();
		} catch (e) {
			l = null;
			error = e instanceof Error ? e.message : String(e);
		}
	}
	$effect(() => {
		id;
		load();
	});

	async function markDone() {
		try {
			const r = await api.post<{ next: string | null }>(`/training/lessons/${id}/complete`);
			l!.done = true;
			if (r.next) goto(`/training/lessons/${r.next}`);
			else goto(`/training/courses/${l!.course.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function markUndone() {
		await api.del(`/training/lessons/${id}/complete`);
		l!.done = false;
	}

	// Self-checks.
	let chosen = $state<Record<string, string[]>>({});
	let checked = $state<Record<string, { correct: boolean; explanation: string | null; right_answers: string[] }>>({});
	function toggle(q: Lesson['self_checks'][number], a: string) {
		const cur = chosen[q.id] ?? [];
		chosen[q.id] = q.kind === 'single' ? [a] : cur.includes(a) ? cur.filter((x) => x !== a) : [...cur, a];
	}
	function move(q: Lesson['self_checks'][number], i: number, by: number) {
		const list = [...q.answers];
		const [x] = list.splice(i, 1);
		list.splice(i + by, 0, x);
		q.answers = list;
	}
	async function check(q: Lesson['self_checks'][number]) {
		try {
			checked[q.id] = await api.post(`/training/questions/${q.id}/check`, { chosen: q.kind === 'order' ? q.answers.map((a) => a.id) : (chosen[q.id] ?? []) });
		} catch (e) {
			toasts.error(e);
		}
	}

	// Editing.
	let f = $state({ title: '', body: '', video_url: '', chapter_id: null as string | null, opens_after_days: 0, file_ids: [] as string[] });
	let files = $state<Lesson['files']>([]);
	let chapters = $state<CourseView['chapters']>([]);
	let preview = $state('');
	async function startEdit() {
		f = { title: l!.title, body: l!.body, video_url: l!.video_url ?? '', chapter_id: l!.chapter_id, opens_after_days: l!.opens_after_days, file_ids: l!.files.map((x) => x.id) };
		files = [...l!.files];
		preview = l!.html;
		editing = true;
		chapters = (await api.get<CourseView>(`/training/courses/${l!.course.id}`)).chapters;
	}
	let t: ReturnType<typeof setTimeout> | undefined;
	function typed() {
		clearTimeout(t);
		t = setTimeout(async () => (preview = (await api.post<{ html: string }>('/kb/preview', { body: f.body })).html), 400);
	}
	async function upload(e: Event) {
		for (const file of Array.from((e.currentTarget as HTMLInputElement).files ?? [])) {
			const fd = new FormData();
			fd.append('file', file, file.name);
			try {
				const up = await api.post<{ id: string; original_name: string; size_bytes: number; mime_type: string }>('/files', fd);
				files = [...files, { id: up.id, name: file.name, size: file.size, mime_type: file.type }];
			} catch (err) {
				toasts.error(err);
			}
		}
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			l = await api.patch<Lesson>(`/training/lessons/${id}`, { ...f, video_url: f.video_url.trim() || null, file_ids: files.map((x) => x.id) });
			editing = false;
			goto(`/training/lessons/${id}`, { replaceState: true });
			toasts.success('Saved');
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove() {
		if (!confirm('Delete this lesson?')) return;
		const course = l!.course.id;
		await api.del(`/training/lessons/${id}`);
		goto(`/training/courses/${course}/edit`);
	}
	const embed = $derived.by(() => {
		const u = l?.video_url ?? '';
		const yt = u.match(/(?:youtube\.com\/watch\?v=|youtu\.be\/)([\w-]{6,})/);
		if (yt) return { kind: 'iframe', src: `https://www.youtube-nocookie.com/embed/${yt[1]}` };
		const vm = u.match(/vimeo\.com\/(\d+)/);
		if (vm) return { kind: 'iframe', src: `https://player.vimeo.com/video/${vm[1]}` };
		if (/\.(mp4|webm|ogg)(\?|$)/i.test(u)) return { kind: 'video', src: u };
		return null;
	});
</script>

<svelte:head><title>{l?.title ?? 'Lesson'} · Training · NERP</title></svelte:head>

{#if error}
	<div class="card max-w-xl p-6"><p class="text-slate-700" data-error>{error[0]?.toUpperCase() + error.slice(1)}.</p><a class="mt-3 inline-block text-brand-600 hover:underline" href="/training">Back to Training</a></div>
{:else if l}
	<div class="max-w-4xl space-y-4">
		<div class="flex flex-wrap items-center gap-3">
			<a href="/training/courses/{l.course.id}" class="flex-1 text-sm text-slate-500 hover:underline">‹ {l.course.title}</a>
			{#if l.can_manage && !editing}<button class="btn btn-secondary text-sm" onclick={startEdit}>Edit</button>{/if}
		</div>

		{#if editing}
			<form class="card space-y-3 p-5" onsubmit={save}>
				<div><label class="label" for="ls-title">Title</label><input id="ls-title" class="input" required maxlength="200" bind:value={f.title} /></div>
				<div class="grid gap-3 sm:grid-cols-2">
					<div><label class="label" for="ls-ch">Chapter</label><select id="ls-ch" class="input" bind:value={f.chapter_id}><option value={null}>—</option>{#each chapters as c (c.id)}<option value={c.id}>{c.title}</option>{/each}</select></div>
					<div><label class="label" for="ls-days">Opens after (days from enrolling)</label><input id="ls-days" class="input" type="number" min="0" max="3650" bind:value={f.opens_after_days} /></div>
				</div>
				<div class="grid gap-3 lg:grid-cols-2">
					<div><label class="label" for="ls-body">Text (Markdown)</label><textarea id="ls-body" class="input min-h-[40vh] font-mono text-sm" bind:value={f.body} oninput={typed}></textarea></div>
					<div><span class="label">Preview</span><div class="prose-kb min-h-[40vh] overflow-auto rounded-xl bg-slate-50 p-3">{@html preview}</div></div>
				</div>
				<div><label class="label" for="ls-video">Video link (YouTube, Vimeo or a video file)</label><input id="ls-video" class="input" type="url" maxlength="500" placeholder="https://" bind:value={f.video_url} /></div>
				<div class="space-y-1">
					<span class="label">Files</span>
					{#each files as x (x.id)}<div class="flex items-center gap-2 text-sm"><span class="flex-1">📎 {x.name}</span><button type="button" class="text-xs text-slate-500 hover:underline" onclick={() => (files = files.filter((y) => y.id !== x.id))}>Remove</button></div>{/each}
					<label class="btn btn-secondary cursor-pointer text-sm">Attach files<input type="file" multiple class="hidden" onchange={upload} /></label>
				</div>
				<p class="text-xs text-slate-500">Self-check questions for this lesson are added under the course's Questions.</p>
				<div class="flex gap-2"><button class="btn btn-primary">Save</button><button type="button" class="btn btn-secondary" onclick={() => (editing = false)}>Cancel</button><div class="flex-1"></div><button type="button" class="btn btn-ghost text-red-600" onclick={remove}>Delete lesson</button></div>
			</form>
		{:else}
			<article class="space-y-4">
				<h1 class="text-2xl font-semibold text-slate-900">{l.title}</h1>
				{#if embed?.kind === 'iframe'}
					<div class="aspect-video overflow-hidden rounded-xl bg-black"><iframe src={embed.src} title={l.title} class="size-full" allow="fullscreen; picture-in-picture" allowfullscreen></iframe></div>
				{:else if embed?.kind === 'video'}
					<video src={embed.src} controls class="w-full rounded-xl bg-black"><track kind="captions" /></video>
				{:else if l.video_url}
					<a class="inline-block text-brand-600 hover:underline" href={l.video_url} target="_blank" rel="noopener noreferrer">▶ Watch the video</a>
				{/if}
				<div class="prose-kb" data-lesson-body>{@html l.html}</div>
				{#if l.files.length}
					<div class="card divide-y divide-slate-50">
						{#each l.files as x (x.id)}<button class="flex w-full items-center gap-2 px-4 py-2.5 text-left text-sm hover:bg-slate-50" onclick={() => downloadFile(x.id, x.name)}>📎 <span class="flex-1">{x.name}</span><span class="text-xs text-slate-400">{Math.ceil(x.size / 1024)} KB</span></button>{/each}
					</div>
				{/if}

				{#each l.self_checks as q (q.id)}
					{@const res = checked[q.id]}
					<div class="card space-y-3 p-4" data-self-check>
						<p class="text-xs font-semibold tracking-wide text-brand-700 uppercase">Check yourself</p>
						<div class="prose-kb">{@html q.html}</div>
						{#if q.kind === 'order'}
							<ol class="space-y-1.5">
								{#each q.answers as a, i (a.id)}
									<li class="flex items-center gap-2 rounded-lg border border-slate-200 px-3 py-2 text-sm"><span class="flex-1">{a.text}</span><button class="btn btn-ghost px-2 text-xs" aria-label="Up" disabled={i === 0} onclick={() => move(q, i, -1)}>↑</button><button class="btn btn-ghost px-2 text-xs" aria-label="Down" disabled={i === q.answers.length - 1} onclick={() => move(q, i, 1)}>↓</button></li>
								{/each}
							</ol>
						{:else}
							<div class="space-y-1.5">
								{#each q.answers as a (a.id)}
									<label class="flex items-center gap-2 rounded-lg border px-3 py-2 text-sm {res && res.right_answers.includes(a.id) ? 'border-emerald-400 bg-emerald-50' : 'border-slate-200'}">
										<input type={q.kind === 'single' ? 'radio' : 'checkbox'} name="sc-{q.id}" checked={(chosen[q.id] ?? []).includes(a.id)} onchange={() => toggle(q, a.id)} />{a.text}
									</label>
								{/each}
							</div>
						{/if}
						<div class="flex items-center gap-3">
							<button class="btn btn-secondary text-sm" onclick={() => check(q)}>Check</button>
							{#if res}<p class="text-sm {res.correct ? 'text-emerald-700' : 'text-amber-800'}" data-result={res.correct ? 'right' : 'wrong'}>{res.correct ? 'Right!' : `Not quite.${res.explanation ? ' ' + res.explanation : ''}`}</p>{/if}
						</div>
					</div>
				{/each}
			</article>

			<div class="flex flex-wrap items-center gap-2 border-t border-slate-100 pt-4">
				{#if l.previous}<a class="btn btn-ghost" href="/training/lessons/{l.previous}">‹ Previous</a>{/if}
				<div class="flex-1"></div>
				{#if l.enrolled}
					{#if l.done}
						<span class="text-sm text-emerald-700" data-done>✓ Done</span><button class="btn btn-ghost text-sm" onclick={markUndone}>Not done</button>
						{#if l.next}<a class="btn btn-primary" href="/training/lessons/{l.next}">Next ›</a>{/if}
					{:else}
						<button class="btn btn-primary" onclick={markDone}>{l.next ? 'Done, next lesson ›' : 'Done'}</button>
					{/if}
				{:else if l.next}
					<a class="btn btn-secondary" href="/training/lessons/{l.next}">Next ›</a>
				{/if}
			</div>
		{/if}
	</div>
{/if}
