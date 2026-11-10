<script lang="ts">
	// One ticket: the conversation (support also sees notes and the history), the reply box,
	// and its details: status, category, who handles it, the SLA deadline.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, downloadFile } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatDateTime } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { CHANNEL_LABEL, dueIn, duration, loadDictionary, nameOf, type Entry, type Sla, type TicketDetail } from '#lib/support.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	const id = $derived(page.params.id!);
	let t = $state<TicketDetail | null>(null);
	let missing = $state(false);
	let dict = $state<Entry[]>([]);
	let team = $state<{ id: string; name: string }[]>([]);
	let slas = $state<Sla[]>([]);
	let answers = $state<{ id: string; title: string; body: string }[]>([]);
	const support = $derived(!!t?.as_support);
	const worker = $derived(session.can(PERM.SUPPORT_WORK) || session.can(PERM.SUPPORT_MANAGE));
	const of = (kind: string) => dict.filter((e) => e.kind === kind);

	async function load() {
		try {
			t = await api.get<TicketDetail>(`/support/tickets/${id}`);
		} catch {
			missing = true;
		}
	}
	onMount(async () => {
		await load();
		try {
			dict = await loadDictionary();
			if (worker) {
				[team, slas, answers] = await Promise.all([
					api.get<{ items: { id: string; name: string }[] }>('/support/team').then((r) => r.items),
					api.get<{ items: Sla[] }>('/support/slas').then((r) => r.items),
					api.get<{ items: { id: string; title: string; body: string }[] }>('/support/answers').then((r) => r.items)
				]);
			}
		} catch (e) {
			toasts.error(e);
		}
	});

	async function change(body: Record<string, unknown>) {
		try {
			t = await api.patch<TicketDetail>(`/support/tickets/${id}`, body);
		} catch (e) {
			toasts.error(e);
			await load();
		}
	}
	async function act(path: string, body?: Record<string, unknown>, done?: string) {
		try {
			t = await api.post<TicketDetail>(`/support/tickets/${id}/${path}`, body);
			if (done) toasts.success(done);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function remove() {
		if (!confirm(`Delete ticket #${t?.number} for good?`)) return;
		try {
			await api.del(`/support/tickets/${id}`);
			await goto('/support');
		} catch (e) {
			toasts.error(e);
		}
	}

	// ---- replying ----
	let body = $state('');
	let note = $state(false);
	let minutes = $state(0);
	let statusId = $state('');
	let files = $state<{ id: string; name: string }[]>([]);
	let fileInput = $state<HTMLInputElement>();
	let sending = $state(false);
	async function attach(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		for (const f of Array.from(input.files ?? [])) {
			const fd = new FormData();
			fd.append('file', f);
			try {
				const meta = await api.post<{ id: string; original_name: string }>('/files', fd);
				files = [...files, { id: meta.id, name: meta.original_name }];
			} catch (err) {
				toasts.error(err);
			}
		}
		input.value = '';
	}
	function useAnswer(e: Event) {
		const sel = e.currentTarget as HTMLSelectElement;
		const a = answers.find((x) => x.id === sel.value);
		if (a) body = body ? `${body}\n\n${a.body}` : a.body;
		sel.value = '';
	}
	async function reply(close = false) {
		if (!body.trim()) return;
		sending = true;
		try {
			t = await api.post<TicketDetail>(`/support/tickets/${id}/messages`, {
				body,
				note: support && note,
				file_ids: files.map((f) => f.id),
				minutes_spent: support ? Number(minutes) || 0 : 0,
				status_id: support && statusId ? statusId : null,
				close: support && close && !note
			});
			body = '';
			files = [];
			minutes = 0;
			statusId = '';
			note = false;
		} catch (e) {
			toasts.error(e);
		} finally {
			sending = false;
		}
	}
	const due = $derived(t && !t.closed_at ? dueIn(t.deadline_at) : null);
</script>

<svelte:head><title>{t ? `#${t.number} ${t.title}` : 'Ticket'} · Help desk · NERP</title></svelte:head>

{#if missing}
	<p class="card px-4 py-10 text-center text-slate-500">This ticket doesn't exist or isn't yours to see.</p>
{:else if t}
	<div class="space-y-4">
		<div>
			<a href="/support" class="text-sm text-slate-500 hover:underline">‹ Help desk</a>
			<h1 class="mt-1 text-2xl font-semibold text-slate-900"><span class="text-slate-400">#{t.number}</span> {t.title}</h1>
			<p class="mt-1 text-sm text-slate-500">
				{t.requester_name ?? t.requester_email} · {CHANNEL_LABEL[t.channel]} · {formatDateTime(t.created_at)}
				{#if t.closed_at}· <span class="font-medium text-slate-700" data-state>Closed{t.auto_closed ? ' automatically' : ''}</span>
				{:else if t.on_hold}· <span class="font-medium text-amber-700" data-state>On hold</span>
				{:else if t.waiting_for === 'customer'}· <span class="font-medium text-sky-700" data-state>{support ? 'Waiting for the customer' : 'Answered'}</span>
				{:else}· <span class="font-medium text-emerald-700" data-state>Waiting for support</span>{/if}
			</p>
		</div>

		<div class="grid gap-4 lg:grid-cols-[1fr_20rem]">
			<div class="min-w-0 space-y-3">
				{#each t.messages as m (m.id)}
					{#if m.kind === 'log'}
						<p class="px-2 text-xs text-slate-500" data-log>{m.body} · {m.author_name} · {formatDateTime(m.created_at)}</p>
					{:else}
						<article class="card p-4 {m.kind === 'note' ? 'border-amber-200 bg-amber-50' : m.by_support ? 'border-brand-100 bg-brand-50/40' : ''}" data-message={m.kind}>
							<header class="flex flex-wrap items-baseline gap-x-2 text-sm">
								<span class="font-medium text-slate-900">{m.author_name}</span>
								{#if m.by_support}<span class="text-xs text-slate-500">Support</span>{/if}
								{#if m.kind === 'note'}<span class="rounded bg-amber-100 px-1.5 text-xs text-amber-800">Note: only support sees it</span>{/if}
								{#if m.is_overdue}<span class="rounded bg-red-50 px-1.5 text-xs text-red-700">Late reply</span>{/if}
								{#if m.email_status === 'failed'}<span class="rounded bg-red-50 px-1.5 text-xs text-red-700">Email not delivered</span>{:else if m.email_status === 'sent'}<span class="text-xs text-slate-400">✉ sent</span>{/if}
								{#if m.minutes_spent}<span class="text-xs text-slate-500">{duration(m.minutes_spent)} spent</span>{/if}
								<span class="ml-auto text-xs text-slate-400">{formatDateTime(m.created_at)}</span>
							</header>
							<p class="mt-2 whitespace-pre-line text-slate-800">{m.body}</p>
							{#if m.files.length}
								<div class="mt-2 flex flex-wrap gap-2">
									{#each m.files as f (f.id)}<button class="rounded-lg border border-slate-200 bg-white px-2.5 py-1 text-xs text-slate-700 hover:bg-slate-50" onclick={() => downloadFile(f.id, f.name)}>📎 {f.name}</button>{/each}
								</div>
							{/if}
						</article>
					{/if}
				{/each}

				{#if !t.is_spam}
					<section class="card space-y-3 p-4">
						{#if t.closed_at && !support}<p class="text-sm text-slate-600">This request is closed. Write again to reopen it.</p>{/if}
						{#if support && answers.length}
							<select class="input w-auto text-sm" aria-label="Insert a frequently used answer" onchange={useAnswer}>
								<option value="">Insert a frequently used answer…</option>
								{#each answers as a (a.id)}<option value={a.id}>{a.title}</option>{/each}
							</select>
						{/if}
						<textarea class="input min-h-28" aria-label={note ? 'Note' : 'Reply'} placeholder={note ? 'A note for support only…' : support ? 'Reply to the requester…' : 'Write a message…'} bind:value={body}></textarea>
						<div class="flex flex-wrap items-center gap-3 text-sm">
							<button type="button" class="btn btn-secondary" onclick={() => fileInput?.click()}><Icon name="paperclip" class="size-4" />Attach</button>
							<input bind:this={fileInput} class="hidden" type="file" multiple aria-label="Attach files" onchange={attach} />
							{#each files as f (f.id)}<span class="rounded-full bg-slate-100 px-2.5 py-1 text-xs">{f.name}</span>{/each}
							{#if support}
								<label class="flex items-center gap-2"><input type="checkbox" bind:checked={note} />Note</label>
								<label class="flex items-center gap-2">Time spent <input class="input w-20" type="number" min="0" max="1440" aria-label="Minutes spent" bind:value={minutes} /> min</label>
								<select class="input w-auto" aria-label="Set status" bind:value={statusId}>
									<option value="">Status unchanged</option>
									{#each of('status') as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
								</select>
							{/if}
							<div class="ml-auto flex gap-2">
								{#if support && !note && !t.closed_at}<button class="btn btn-secondary" disabled={sending || !body.trim()} onclick={() => reply(true)}>Send and close</button>{/if}
								<button class="btn btn-primary" disabled={sending || !body.trim()} onclick={() => reply()}>{note ? 'Add note' : 'Send'}</button>
							</div>
						</div>
					</section>
				{/if}
			</div>

			<aside class="space-y-4">
				{#if t.can_rate}
					<section class="card space-y-2 p-4" data-rate>
						<h2 class="font-medium text-slate-900">How was the answer?</h2>
						{#each of('mark') as m (m.id)}
							<button class="btn w-full justify-start {t.mark_id === m.id ? 'btn-primary' : 'btn-secondary'}" onclick={() => act('rate', { mark_id: m.id }, 'Thank you')}>{m.name}</button>
						{/each}
					</section>
				{/if}
				<section class="card space-y-3 p-4 text-sm">
					{#if support}
						{#if due}<p class={due.late || t.is_overdue ? 'font-medium text-red-600' : 'text-slate-700'} data-deadline>Reply {due.text} ({formatDateTime(t.deadline_at)})</p>{/if}
						{#each [['status', 'status_id'], ['category', 'category_id'], ['criticality', 'criticality_id'], ['difficulty', 'difficulty_id']] as [kind, field] (kind)}
							<div>
								<label class="label capitalize" for="f-{kind}">{kind}</label>
								<select id="f-{kind}" class="input" value={(t as unknown as Record<string, string | null>)[field] ?? ''} onchange={(e) => change({ [field]: e.currentTarget.value || null })}>
									<option value="" selected={!(t as unknown as Record<string, string | null>)[field]}>—</option>
									{#each of(kind) as o (o.id)}<option value={o.id} selected={o.id === (t as unknown as Record<string, string | null>)[field]}>{o.name}</option>{/each}
								</select>
							</div>
						{/each}
						<div>
							<label class="label" for="f-resp">Responsible</label>
							<select id="f-resp" class="input" value={t.responsible_id ?? ''} onchange={(e) => change({ responsible_id: e.currentTarget.value || null })}>
								<option value="" selected={!t.responsible_id}>Nobody</option>
								{#each team as u (u.id)}<option value={u.id} selected={u.id === t.responsible_id}>{u.name}</option>{/each}
							</select>
						</div>
						<div>
							<label class="label" for="f-sla">SLA</label>
							<select id="f-sla" class="input" value={t.sla_id ?? ''} onchange={(e) => e.currentTarget.value && change({ sla_id: e.currentTarget.value })}>
								{#each slas as s (s.id)}<option value={s.id} selected={s.id === t.sla_id}>{s.name} ({duration(s.response_minutes)})</option>{/each}
							</select>
						</div>
						<label class="flex items-center gap-2"><input type="checkbox" checked={t.on_hold} onchange={(e) => change({ on_hold: e.currentTarget.checked })} />On hold (the clock stops)</label>
						<dl class="space-y-1 text-slate-600">
							<div class="flex justify-between"><dt>From</dt><dd class="text-right">{t.requester_name}{#if t.requester_email}<br /><span class="text-xs">{t.requester_email}</span>{/if}</dd></div>
							{#if t.contact_id}<div class="flex justify-between"><dt>CRM</dt><dd><a class="text-brand-700 hover:underline" href="/crm/contact/{t.contact_id}">Contact</a></dd></div>{/if}
							<div class="flex justify-between"><dt>First reply</dt><dd>{t.first_response_at ? duration((Date.parse(t.first_response_at) - Date.parse(t.created_at)) / 60000) : '—'}</dd></div>
							<div class="flex justify-between"><dt>Time spent</dt><dd data-spent>{duration(t.minutes_spent ?? 0)}</dd></div>
							{#if t.overdue_count}<div class="flex justify-between"><dt>Times overdue</dt><dd>{t.overdue_count}</dd></div>{/if}
						</dl>
					{:else}
						<dl class="space-y-1 text-slate-600">
							<div class="flex justify-between"><dt>Status</dt><dd>{nameOf(dict, t.status_id) || '—'}</dd></div>
							<div class="flex justify-between"><dt>Category</dt><dd>{nameOf(dict, t.category_id) || '—'}</dd></div>
							<div class="flex justify-between"><dt>Handled by</dt><dd>{t.responsible_name ?? 'Not assigned yet'}</dd></div>
						</dl>
					{/if}
					<div class="flex flex-wrap gap-2 border-t border-slate-100 pt-3">
						{#if t.closed_at}<button class="btn btn-secondary" onclick={() => act('reopen', undefined, 'Reopened')}>Reopen</button>
						{:else}<button class="btn btn-secondary" onclick={() => act('close', undefined, 'Closed')}>Close</button>{/if}
						{#if support}<button class="btn btn-secondary" onclick={() => change({ is_spam: !t?.is_spam })}>{t.is_spam ? 'Not spam' : 'Spam'}</button>{/if}
						{#if session.can(PERM.SUPPORT_MANAGE)}<button class="btn btn-secondary text-red-700" onclick={remove}><Icon name="trash" class="size-4" />Delete</button>{/if}
					</div>
				</section>
			</aside>
		</div>
	</div>
{/if}
