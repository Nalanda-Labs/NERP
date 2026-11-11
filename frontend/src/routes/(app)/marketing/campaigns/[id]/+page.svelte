<script lang="ts">
	// One campaign: written and sent from here while it is a draft; afterwards, its results
	// (delivery, opens, clicks, links, what it brought in) and its recipients.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import EmailDesigner, { blankDesign } from '#lib/components/marketing/EmailDesigner.svelte';
	import { crm, money } from '#lib/crm.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { CAMPAIGN_STATUS, rate, type Campaign } from '../campaign.ts';

	type Segment = { id: string; name: string; reachable: number; type_code: string };
	type Stats = {
		counts: Campaign['counts']; skipped: number; utm_campaign: string;
		links: { url: string; clicks: number; people: number }[];
		brought: { type: string; count: number; won_amount: string | null }[];
	};
	type Recipient = { id: string; item_id: string | null; title: string | null; type_code: string | null; email: string; status: string; error: string | null; sent_at: string | null; opened_at: string | null; clicked_at: string | null; unsubscribed_at: string | null };

	const id = $derived(page.params.id!);
	let c = $state<Campaign | null>(null);
	let segments = $state<Segment[]>([]);
	let stats = $state<Stats | null>(null);
	let saving = $state(false);
	let dirty = $state(false);

	async function load() {
		try {
			c = await api.get<Campaign>(`/marketing/campaigns/${id}`);
			dirty = false;
			if (c.status === 'draft') segments = (await api.get<{ items: Segment[] }>('/marketing/segments')).items;
			else {
				stats = await api.get<Stats>(`/marketing/campaigns/${id}/stats`);
				await loadRecipients();
			}
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(() => {
		crm.load();
		load();
		// While sending, refresh now and then.
		const t = setInterval(() => c && (c.status === 'sending' || c.status === 'scheduled') && load(), 4000);
		return () => clearInterval(t);
	});

	// ----- editing a draft -----
	async function save() {
		if (!c) return;
		saving = true;
		try {
			c = await api.patch<Campaign>(`/marketing/campaigns/${id}`, { name: c.name, segment_ids: c.segment_ids, subject: c.subject, body: c.body, design: c.design ? $state.snapshot(c.design) : null, from_name: c.from_name || null, reply_to: c.reply_to || null });
			dirty = false;
			toasts.success('Saved');
		} catch (e) {
			toasts.error(e);
		} finally {
			saving = false;
		}
	}
	function setDesigned(on: boolean) {
		if (!c || on === !!c.design) return;
		if (!on && !confirm('Go back to a plain-text message? The design is dropped; its text stays.')) return;
		c.design = on ? blankDesign() : null;
		dirty = true;
	}
	const reach = $derived(segments.filter((s) => c?.segment_ids.includes(s.id)).reduce((a, s) => a + s.reachable, 0));
	// ----- starting points for the message -----
	const READY: { name: string; subject: string; body: string }[] = [
		{ name: 'Newsletter', subject: 'News from us this month', body: 'Hello {{first_name}},\n\nHere is what is new this month:\n\n• …\n• …\n• …\n\nRead more: https://yourcompany.com/news\n\nBest regards,\n{{assigned}}' },
		{ name: 'Special offer', subject: '{{first_name}}, an offer for you', body: 'Hello {{first_name}},\n\nUntil the end of the month you get … off …\n\nSee the offer: https://yourcompany.com/offer\n\nQuestions? Just reply to this email.\n\n{{assigned}}' },
		{ name: 'Event invitation', subject: 'You are invited: …', body: 'Hello {{first_name}},\n\nWe would like to invite you to … on … at ….\n\nWhat to expect:\n• …\n• …\n\nSave your seat: https://yourcompany.com/event\n\nSee you there,\n{{assigned}}' },
		{ name: 'Welcome', subject: 'Welcome, {{first_name}}', body: 'Hello {{first_name}},\n\nThank you for choosing us. Here is how to get started:\n\n1. …\n2. …\n\nYour contact person is {{assigned}} ({{assigned_email}}).\n\nWelcome aboard!' },
		{ name: 'Win back', subject: 'We miss you, {{first_name}}', body: 'Hello {{first_name}},\n\nIt has been a while. Here is what changed since your last visit: …\n\nCome back and take a look: https://yourcompany.com\n\n{{assigned}}' },
		{ name: 'Feedback request', subject: 'How did we do?', body: 'Hello {{first_name}},\n\nWe would love to hear how things went. It takes a minute:\n\nhttps://yourcompany.com/feedback\n\nThank you,\n{{assigned}}' }
	];
	type Source = { key: string; name: string; subject: string; body: string };
	let mine = $state<Source[]>([]);
	let previous = $state<Source[]>([]);
	let startFrom = $state('');
	async function loadSources() {
		const [t, p] = await Promise.all([
			api.get<{ items: { id: string; name: string; subject: string; body: string }[] }>('/crm/email-templates').catch(() => ({ items: [] })),
			api.get<{ items: Campaign[] }>('/marketing/campaigns').catch(() => ({ items: [] as Campaign[] }))
		]);
		mine = t.items.map((x) => ({ key: `t:${x.id}`, name: x.name, subject: x.subject, body: x.body }));
		previous = p.items.filter((x) => x.id !== id && x.body.trim()).map((x) => ({ key: `c:${x.id}`, name: x.name, subject: x.subject, body: x.body }));
	}
	function applySource(key: string) {
		const src = [...READY.map((x, i) => ({ ...x, key: `r:${i}` })), ...mine, ...previous].find((x) => x.key === key);
		startFrom = '';
		if (!src || !c) return;
		if (c.body.trim() && !confirm('Replace the subject and message with this one?')) return;
		c.subject = src.subject;
		c.body = src.body;
		dirty = true;
	}
	async function saveAsTemplate() {
		if (!c) return;
		const name = prompt('Name the template', c.name);
		if (!name?.trim()) return;
		try {
			await api.post('/crm/email-templates', { name, subject: c.subject, body: c.body });
			toasts.success('Saved as a template');
			await loadSources();
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(loadSources);

	const PLACEHOLDERS = ['first_name', 'last_name', 'title', 'company', 'assigned', 'assigned_email'];
	let bodyEl = $state<HTMLTextAreaElement | null>(null);
	function insert(ph: string) {
		if (!c) return;
		const at = bodyEl?.selectionStart ?? c.body.length;
		c.body = c.body.slice(0, at) + `{{${ph}}}` + c.body.slice(at);
		dirty = true;
	}
	let testTo = $state(session.user?.email ?? '');
	async function testSend() {
		if (dirty) await save();
		try {
			await api.post(`/marketing/campaigns/${id}/test`, { email: testTo });
			toasts.success(`Test sent to ${testTo}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	let when = $state<'now' | 'later'>('now');
	let sendAt = $state('');
	async function send() {
		if (!c) return;
		if (dirty) await save();
		const later = when === 'later' && sendAt;
		if (!confirm(later ? `Send “${c.name}” on ${new Date(sendAt).toLocaleString()}?` : `Send “${c.name}” to about ${reach} people now?`)) return;
		try {
			await api.post(`/marketing/campaigns/${id}/send`, later ? { send_at: new Date(sendAt).toISOString() } : {});
			toasts.success(later ? 'Scheduled' : 'Sending started');
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function cancel() {
		if (!c || !confirm(c.status === 'scheduled' ? 'Cancel the schedule? It goes back to a draft.' : 'Stop sending? Emails already sent stay sent.')) return;
		try {
			await api.post(`/marketing/campaigns/${id}/cancel`, {});
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function remove() {
		if (!c || !confirm(`Delete “${c.name}” and its results?`)) return;
		try {
			await api.del(`/marketing/campaigns/${id}`);
			goto('/marketing/campaigns');
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- results -----
	let recipients = $state<Recipient[]>([]);
	let rFilter = $state('');
	async function loadRecipients() {
		recipients = (await api.get<{ items: Recipient[] }>(`/marketing/campaigns/${id}/recipients?limit=200${rFilter ? `&status=${rFilter}` : ''}`)).items;
	}
	const funnel = $derived(
		c ? [
			['Sent', c.counts.sent],
			['Opened', c.counts.opened],
			['Clicked', c.counts.clicked]
		] as [string, number][] : []
	);
	const TYPE_LABEL: Record<string, string> = { lead: 'Leads', deal: 'Deals', contact: 'Contacts', company: 'Companies' };
</script>

<svelte:head><title>{c?.name ?? 'Campaign'} · NERP</title></svelte:head>

{#if c}
	{@const st = CAMPAIGN_STATUS[c.status]}
	<div class="space-y-5">
		<div>
			<a href="/marketing/campaigns" class="text-sm text-slate-500 hover:underline">← Campaigns</a>
			<div class="mt-1 flex flex-wrap items-center gap-3">
				{#if c.status === 'draft'}
					<input class="input max-w-md text-xl font-semibold" bind:value={c.name} oninput={() => (dirty = true)} aria-label="Campaign name" />
				{:else}
					<h1 class="text-2xl font-semibold text-slate-900">{c.name}</h1>
				{/if}
				<span class="rounded-full px-2.5 py-0.5 text-xs font-medium {st?.[1]}" data-status={c.status}>{st?.[0]}</span>
				<div class="ml-auto flex gap-2">
					{#if c.status === 'scheduled' || c.status === 'sending'}<button class="btn btn-secondary" onclick={cancel}>{c.status === 'scheduled' ? 'Cancel schedule' : 'Stop sending'}</button>{/if}
					{#if c.status !== 'sending'}<button class="btn btn-ghost text-red-600" onclick={remove}><Icon name="trash" class="size-4" /> Delete</button>{/if}
				</div>
			</div>
			{#if c.status === 'scheduled'}<p class="mt-1 text-sm text-slate-600">Goes out {formatDateTime(c.send_at)}.</p>{/if}
		</div>

		{#if c.status === 'draft'}
			<div class="grid gap-5 {c.design ? 'xl:grid-cols-1' : 'lg:grid-cols-[minmax(0,1fr)_20rem]'}">
				<section class="card space-y-4 p-5 text-sm">
					<div class="grid gap-3 sm:grid-cols-2">
						<div><label class="label" for="cp-from">Sender name</label><input id="cp-from" class="input" maxlength="100" bind:value={c.from_name} oninput={() => (dirty = true)} placeholder="e.g. Acme team" /></div>
						<div><label class="label" for="cp-reply">Replies go to</label><input id="cp-reply" class="input" type="email" bind:value={c.reply_to} oninput={() => (dirty = true)} placeholder="sales@yourcompany.com" /></div>
					</div>
					<div><label class="label" for="cp-subject">Subject</label><input id="cp-subject" class="input" maxlength="300" bind:value={c.subject} oninput={() => (dirty = true)} /></div>
					<div class="flex rounded-full bg-slate-100 p-0.5 text-xs w-fit" role="tablist" aria-label="Message kind">
						{#each [[false, 'Plain text'], [true, 'Designed']] as const as [on, l] (l)}
							<button type="button" role="tab" aria-selected={!!c.design === on} class="rounded-full px-3 py-1 {!!c.design === on ? 'bg-white font-medium text-slate-900 shadow-sm' : 'text-slate-600'}" onclick={() => setDesigned(on)} data-message-kind={on ? 'designed' : 'plain'}>{l}</button>
						{/each}
					</div>
					{#if c.design}
						<EmailDesigner bind:design={c.design} onchange={() => (dirty = true)} />
						<p class="text-xs text-slate-500">Links are tracked and get UTM tags (<code>utm_campaign={c.slug}</code>). An unsubscribe line is added at the end. A plain-text version goes with it.</p>
					{:else}
					<div>
						<div class="flex flex-wrap items-center gap-2">
							<label class="label mr-auto" for="cp-body">Message</label>
							<select class="input w-48 py-1 text-xs" aria-label="Start from" bind:value={startFrom} onchange={() => applySource(startFrom)} data-start-from>
								<option value="">Start from…</option>
								<optgroup label="Ready-made">{#each READY as t, i (i)}<option value="r:{i}">{t.name}</option>{/each}</optgroup>
								{#if mine.length}<optgroup label="Email templates">{#each mine as t (t.key)}<option value={t.key}>{t.name}</option>{/each}</optgroup>{/if}
								{#if previous.length}<optgroup label="Earlier campaigns">{#each previous as t (t.key)}<option value={t.key}>{t.name}</option>{/each}</optgroup>{/if}
							</select>
							<button type="button" class="btn btn-ghost px-2 py-1 text-xs" disabled={!c.body.trim()} onclick={saveAsTemplate}>Save as template</button>
						</div>
						<textarea id="cp-body" class="input font-mono text-[13px]" rows="14" bind:this={bodyEl} bind:value={c.body} oninput={() => (dirty = true)} placeholder={'Hello {{first_name}},\n\n…\n\nhttps://yourcompany.com/offer'}></textarea>
						<div class="mt-1 flex flex-wrap gap-1">
							{#each PLACEHOLDERS as ph (ph)}<button type="button" class="rounded bg-slate-100 px-1.5 py-0.5 font-mono text-[11px] text-slate-600 hover:bg-brand-50 hover:text-brand-700" onclick={() => insert(ph)}>{`{{${ph}}}`}</button>{/each}
						</div>
						<p class="mt-1 text-xs text-slate-500">Links are tracked and get UTM tags (<code>utm_campaign={c.slug}</code>), so leads and deals they bring in are counted. An unsubscribe link is added at the end.</p>
					</div>
					{/if}
					<div class="flex gap-2"><button class="btn btn-primary" disabled={saving || !dirty} onclick={save}>{saving ? 'Saving…' : dirty ? 'Save' : 'Saved'}</button></div>
				</section>
				<aside class="space-y-4">
					<section class="card space-y-3 p-5 text-sm" aria-labelledby="who-h">
						<h2 id="who-h" class="font-medium text-slate-900">Who gets it</h2>
						{#each segments as s (s.id)}
							<label class="flex items-center gap-2">
								<input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={c.segment_ids.includes(s.id)} onchange={(e) => ((c!.segment_ids = e.currentTarget.checked ? [...c!.segment_ids, s.id] : c!.segment_ids.filter((x) => x !== s.id)), (dirty = true))} />
								<span class="flex-1">{s.name}</span><span class="text-xs text-slate-500 tabular-nums">{s.reachable}</span>
							</label>
						{:else}
							<p class="text-slate-500">No segments yet. <a class="text-brand-700 hover:underline" href="/marketing/segments">Make one</a>.</p>
						{/each}
						<p class="border-t border-slate-100 pt-2 text-xs text-slate-500">About <strong class="text-slate-800" data-reach>{reach}</strong> people (each address gets one email; unsubscribed ones are left out).</p>
					</section>
					<section class="card space-y-2 p-5 text-sm" aria-labelledby="test-h">
						<h2 id="test-h" class="font-medium text-slate-900">Send a test</h2>
						<div class="flex gap-2"><input class="input" type="email" bind:value={testTo} aria-label="Test address" /><button class="btn btn-secondary" disabled={!testTo} onclick={testSend}>Send</button></div>
						<p class="text-xs text-slate-500">Filled in from the first person in the audience.</p>
					</section>
					<section class="card space-y-3 p-5 text-sm" aria-labelledby="send-h">
						<h2 id="send-h" class="font-medium text-slate-900">Send</h2>
						<label class="flex items-center gap-2"><input type="radio" class="text-brand-600" value="now" bind:group={when} /> Now</label>
						<label class="flex items-center gap-2"><input type="radio" class="text-brand-600" value="later" bind:group={when} /> Later</label>
						{#if when === 'later'}<input class="input" type="datetime-local" bind:value={sendAt} aria-label="Send at" />{/if}
						<button class="btn btn-primary w-full" disabled={!c.segment_ids.length || !c.subject.trim() || !c.body.trim() || (when === 'later' && !sendAt)} onclick={send}><Icon name="send" class="size-4" /> {when === 'later' ? 'Schedule' : 'Send now'}</button>
					</section>
				</aside>
			</div>
		{:else if stats}
			<section class="grid gap-3 sm:grid-cols-3 lg:grid-cols-6" aria-label="Results">
				{#each [['Recipients', c.counts.recipients, ''], ['Sent', c.counts.sent, c.counts.pending ? `${c.counts.pending} waiting` : ''], ['Opened', c.counts.opened, rate(c.counts.opened, c.counts.sent)], ['Clicked', c.counts.clicked, rate(c.counts.clicked, c.counts.sent)], ['Unsubscribed', c.counts.unsubscribed, rate(c.counts.unsubscribed, c.counts.sent)], ['Failed', c.counts.failed, stats.skipped ? `${stats.skipped} left out` : '']] as [label, n, sub] (label)}
					<div class="card p-4" data-tile={label}>
						<p class="text-xs text-slate-500">{label}</p>
						<p class="text-2xl font-semibold text-slate-900 tabular-nums">{n}</p>
						<p class="text-xs text-slate-500">{sub}</p>
					</div>
				{/each}
			</section>
			<div class="grid gap-5 lg:grid-cols-2">
				<section class="card p-5" aria-labelledby="fun-h">
					<h2 id="fun-h" class="mb-3 font-medium text-slate-900">From sent to clicked</h2>
					<ul class="space-y-2.5">
						{#each funnel as [label, n] (label)}
							<li class="grid grid-cols-[5rem_1fr_6rem] items-center gap-3 text-sm">
								<span class="text-slate-600">{label}</span>
								<div class="h-3 rounded-full bg-slate-100"><div class="h-3 rounded-full" style="width:{c.counts.sent && n ? Math.max(1, (n / c.counts.sent) * 100) : 0}%;background:var(--series-1)"></div></div>
								<span class="text-right text-slate-800 tabular-nums">{n} · {rate(n, c.counts.sent)}</span>
							</li>
						{/each}
					</ul>
				</section>
				<section class="card p-5" aria-labelledby="brought-h">
					<h2 id="brought-h" class="mb-1 font-medium text-slate-900">What it brought in</h2>
					<p class="mb-3 text-xs text-slate-500">Records whose campaign tag is <code>{stats.utm_campaign}</code> (from web forms reached through its links).</p>
					{#if stats.brought.length}
						<ul class="space-y-1 text-sm">
							{#each stats.brought as b (b.type)}<li class="flex justify-between"><span>{TYPE_LABEL[b.type] ?? b.type}</span><span class="tabular-nums">{b.count}{b.won_amount ? ` · won ${money(b.won_amount, crm.config?.base_currency)}` : ''}</span></li>{/each}
						</ul>
					{:else}
						<p class="text-sm text-slate-500">Nothing yet.</p>
					{/if}
				</section>
			</div>
			<section class="card overflow-x-auto" aria-labelledby="links-h">
				<h2 id="links-h" class="border-b border-slate-100 px-5 py-3 font-medium text-slate-900">Links</h2>
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500"><tr><th class="px-5 py-2 font-medium">Link</th><th class="px-3 py-2 text-right font-medium">Clicks</th><th class="px-5 py-2 text-right font-medium">People</th></tr></thead>
					<tbody class="divide-y divide-slate-50">
						{#each stats.links as l (l.url)}<tr><td class="max-w-md truncate px-5 py-2 font-mono text-xs" title={l.url}>{l.url}</td><td class="px-3 py-2 text-right tabular-nums">{l.clicks}</td><td class="px-5 py-2 text-right tabular-nums">{l.people}</td></tr>{:else}<tr><td colspan="3" class="px-5 py-4 text-slate-500">No clicks yet.</td></tr>{/each}
					</tbody>
				</table>
			</section>
			<section class="card overflow-x-auto" aria-labelledby="rcp-h">
				<div class="flex items-center gap-3 border-b border-slate-100 px-5 py-3">
					<h2 id="rcp-h" class="flex-1 font-medium text-slate-900">Recipients</h2>
					<select class="input w-40" bind:value={rFilter} onchange={loadRecipients} aria-label="Show">
						<option value="">Everyone</option><option value="opened">Opened</option><option value="clicked">Clicked</option><option value="unsubscribed">Unsubscribed</option><option value="failed">Failed</option><option value="pending">Waiting</option>
					</select>
				</div>
				<table class="w-full text-sm">
					<tbody class="divide-y divide-slate-50">
						{#each recipients as r (r.id)}
							<tr data-recipient={r.email}>
								<td class="px-5 py-2">{#if r.item_id && r.type_code}<a class="text-brand-700 hover:underline" href="/crm/{r.type_code}/{r.item_id}">{r.title}</a>{:else}{r.title ?? '—'}{/if}<span class="block text-xs text-slate-500">{r.email}</span></td>
								<td class="px-3 py-2 text-xs text-slate-600">
									{#if r.status === 'failed'}<span class="text-red-700">Failed: {r.error}</span>
									{:else if r.status === 'pending'}Waiting
									{:else}{[r.opened_at && 'opened', r.clicked_at && 'clicked', r.unsubscribed_at && 'unsubscribed'].filter(Boolean).join(', ') || 'sent'}{/if}
								</td>
								<td class="px-5 py-2 text-right text-xs text-slate-500">{formatDateTime(r.sent_at)}</td>
							</tr>
						{:else}
							<tr><td class="px-5 py-4 text-slate-500">Nobody here.</td></tr>
						{/each}
					</tbody>
				</table>
			</section>
		{/if}
	</div>
{:else}
	<p class="text-sm text-slate-400">Loading…</p>
{/if}
