<script lang="ts">
	// One email, opened from the timeline. The HTML body (already sanitised on the server)
	// renders in a sandboxed frame that may not run scripts or load anything, so remote
	// images and tracking pixels stay blocked.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api, downloadFile } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';
	import type { ReplyTarget } from './EmailComposer.svelte';

	type Addr = { address: string; name: string };
	type Email = {
		id: string;
		direction: 'in' | 'out';
		status: string;
		from_addr: string;
		from_name: string;
		to_addrs: Addr[];
		cc_addrs: Addr[];
		subject: string;
		body_text: string | null;
		body_html: string | null;
		attachments: { file_id: string; name: string; size: number }[];
		sent_at: string;
		error: string | null;
	};

	let { emailId, onreply }: { emailId: string; onreply?: (r: ReplyTarget) => void } = $props();
	let email = $state<Email | null>(null);
	let height = $state(200);

	async function load() {
		try {
			email = await api.get<Email>(`/crm/emails/${emailId}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);

	// Remote images (often tracking pixels) load only when asked for.
	let showImages = $state(false);
	const remote = /(<img\b[^>]*?\s)src=(["'])(https?:)/gi;
	const hasRemote = $derived(!!email?.body_html && /<img\b[^>]*?\ssrc=["']https?:/i.test(email.body_html));
	const csp = $derived(`<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data: ${showImages ? 'https:' : ''}; style-src 'unsafe-inline'">`);
	const html = $derived(email?.body_html ? (showImages ? email.body_html : email.body_html.replace(remote, '$1data-src=$2$3')) : '');
	const srcdoc = $derived(html ? `<!doctype html><html><head>${csp}<base target="_blank"><style>body{font-family:system-ui,sans-serif;font-size:14px;margin:0;color:#1e293b;word-wrap:break-word}</style></head><body>${html}</body></html>` : '');
	const people = (l: Addr[]) => l.map((a) => (a.name ? `${a.name} <${a.address}>` : a.address)).join(', ');

	// ----- a task, an event, or a discussion from the email -----
	async function makeTask() {
		try {
			const r = await api.post<{ task_id: string; number: number }>(`/crm/emails/${emailId}/task`, {});
			toasts.success(`Task #${r.number} made`);
			goto(`/tasks/${r.task_id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	let eventOpen = $state(false);
	let ev = $state({ date: new Date(Date.now() + 86400e3).toISOString().slice(0, 10), time: '10:00', minutes: 30 });
	async function makeEvent(e: SubmitEvent) {
		e.preventDefault();
		const start = new Date(`${ev.date}T${ev.time}:00`);
		try {
			await api.post(`/crm/emails/${emailId}/event`, { start: start.toISOString(), end: new Date(+start + ev.minutes * 60e3).toISOString(), timezone: Intl.DateTimeFormat().resolvedOptions().timeZone });
			eventOpen = false;
			toasts.success('The event is on your calendar');
		} catch (err) {
			toasts.error(err);
		}
	}
	let shareOpen = $state(false);
	let share = $state({ where: 'feed', chat_id: '', comment: '' });
	let chats = $state<{ id: string; title: string }[]>([]);
	$effect(() => {
		if (shareOpen && !chats.length) api.get<{ items: { id: string; title: string }[] }>('/im/chats').then((r) => (chats = r.items));
	});
	async function doShare(e: SubmitEvent) {
		e.preventDefault();
		try {
			const body = share.where === 'feed' ? { feed: true, comment: share.comment || null } : { chat_id: share.chat_id, comment: share.comment || null };
			const r = await api.post<{ chat_id?: string; post_id?: string }>(`/crm/emails/${emailId}/share`, body);
			shareOpen = false;
			toasts.success(r.chat_id ? 'Posted in the chat' : 'Posted in the feed');
		} catch (err) {
			toasts.error(err);
		}
	}

	async function resend() {
		try {
			await api.post(`/crm/emails/${emailId}/resend`);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

{#if email}
	<div class="mt-2 rounded-xl border border-slate-200 bg-white p-3 text-sm">
		<div class="space-y-0.5 text-xs text-slate-500">
			<p><span class="text-slate-400">From</span> {email.from_name ? `${email.from_name} <${email.from_addr}>` : email.from_addr}</p>
			<p><span class="text-slate-400">To</span> {people(email.to_addrs)}</p>
			{#if email.cc_addrs.length}<p><span class="text-slate-400">Cc</span> {people(email.cc_addrs)}</p>{/if}
			<p>{formatDateTime(email.sent_at)}{#if email.status === 'queued'} · <span class="text-amber-700">sending…</span>{:else if email.status === 'failed'} · <span class="text-red-700">not sent: {email.error}</span>{/if}</p>
		</div>
		{#if hasRemote && !showImages}
			<button class="mt-2 text-xs text-brand-600 hover:underline" onclick={() => (showImages = true)}>Remote images are hidden for privacy · Show images</button>
		{/if}
		<div class="mt-3">
			{#if email.body_html}
				<iframe title="Email body" sandbox="allow-popups allow-popups-to-escape-sandbox allow-same-origin" {srcdoc} class="w-full border-0" style="height:{height}px" onload={(e) => { const d = (e.currentTarget as HTMLIFrameElement).contentDocument; if (d) height = Math.min(Math.max(d.body.scrollHeight + 16, 60), 800); }}></iframe>
			{:else}
				<pre class="font-sans text-sm whitespace-pre-wrap text-slate-800">{email.body_text}</pre>
			{/if}
		</div>
		{#if email.attachments.length}
			<div class="mt-2 flex flex-wrap gap-2">
				{#each email.attachments as a (a.file_id)}
					<button class="badge flex items-center gap-1 bg-slate-100 text-slate-700 hover:bg-slate-200" onclick={() => downloadFile(a.file_id, a.name).catch(toasts.error)}><Icon name="paperclip" class="size-3" />{a.name} <span class="text-slate-400">{Math.ceil(a.size / 1024)} KB</span></button>
				{/each}
			</div>
		{/if}
		<div class="mt-2 flex gap-2">
			{#if onreply}
				<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => onreply({ id: email!.id, from_addr: email!.direction === 'in' ? email!.from_addr : (email!.to_addrs[0]?.address ?? ''), subject: email!.subject, cc: email!.cc_addrs.map((c) => c.address) })}><Icon name="send" class="size-3.5" /> Reply</button>
			{/if}
			{#if email.status === 'failed'}<button class="btn btn-ghost px-2 py-1 text-xs" onclick={resend}><Icon name="refresh" class="size-3.5" /> Send again</button>{/if}
			<button class="btn btn-ghost px-2 py-1 text-xs" onclick={makeTask} data-email-task><Icon name="checkSquare" class="size-3.5" /> Make a task</button>
			<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => (eventOpen = true)}><Icon name="calendar" class="size-3.5" /> Plan an event</button>
			<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => (shareOpen = true)} data-email-share><Icon name="message" class="size-3.5" /> Discuss</button>
		</div>
	</div>
{/if}

<Modal bind:open={eventOpen} title="Plan an event">
	<form id="email-event" class="grid gap-3 sm:grid-cols-3" onsubmit={makeEvent}>
		<div><label class="label" for="ee-d">On</label><input id="ee-d" type="date" class="input" required bind:value={ev.date} /></div>
		<div><label class="label" for="ee-t">At</label><input id="ee-t" type="time" class="input" required bind:value={ev.time} /></div>
		<div><label class="label" for="ee-m">Minutes</label><input id="ee-m" type="number" min="5" class="input" required bind:value={ev.minutes} /></div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (eventOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="email-event">Add to calendar</button>
	{/snippet}
</Modal>

<Modal bind:open={shareOpen} title="Discuss the email">
	<form id="email-share" class="space-y-3" onsubmit={doShare}>
		<div class="flex gap-4 text-sm">
			<label class="flex items-center gap-2"><input type="radio" value="feed" bind:group={share.where} /> In the news feed</label>
			<label class="flex items-center gap-2"><input type="radio" value="chat" bind:group={share.where} /> In a chat</label>
		</div>
		{#if share.where === 'chat'}
			<select class="input" aria-label="Chat" required bind:value={share.chat_id}>
				<option value="" disabled>Choose a chat…</option>
				{#each chats as c (c.id)}<option value={c.id}>{c.title}</option>{/each}
			</select>
		{/if}
		<textarea class="input" rows="2" placeholder="Add a few words (optional)" aria-label="Comment" bind:value={share.comment}></textarea>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (shareOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="email-share">Post</button>
	{/snippet}
</Modal>
