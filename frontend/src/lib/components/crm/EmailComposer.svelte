<script lang="ts" module>
	import { api } from '#lib/api.ts';

	export type MailboxInfo = { id: string; email: string; display_name: string; is_owner: boolean; last_error: string | null };
	let cached: Promise<MailboxInfo[]> | null = null;
	/** Mailboxes the current user can send from (cached for the session). */
	export function myMailboxes(force = false): Promise<MailboxInfo[]> {
		if (force || !cached) cached = api.get<{ items: MailboxInfo[] }>('/crm/mailboxes').then((r) => r.items).catch(() => []);
		return cached;
	}
	export type ReplyTarget = { id: string; from_addr: string; subject: string; cc?: string[] };
</script>

<script lang="ts">
	// Writing an email from a CRM record (or replying within a conversation).
	import { onMount } from 'svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';

	let { itemId, entityType, reply = null, onsent, oncancel }: { itemId: string; entityType?: number; reply?: ReplyTarget | null; onsent?: () => void; oncancel?: () => void } = $props();

	// Email templates: shared ones and the user's own, filled in for this record.
	type Template = { id: string; name: string; subject: string; body: string; owner_id: string | null; entity_types: number[] };
	let templates = $state<Template[]>([]);
	let canShare = $state(false);
	let templateId = $state('');
	async function loadTemplates() {
		const r = await api.get<{ items: Template[]; can_share: boolean }>(`/crm/email-templates${entityType ? `?entity_type=${entityType}` : ''}`).catch(() => null);
		templates = r?.items ?? [];
		canShare = r?.can_share ?? false;
	}
	async function useTemplate(id: string) {
		templateId = id;
		if (!id) return;
		if ((subject.trim() || body.trim()) && !confirm('Replace the subject and message with the template?')) return;
		try {
			const r = await api.post<{ subject: string; body: string }>(`/crm/email-templates/${id}/render`, { item_id: itemId });
			if (!reply || !subject.trim()) subject = r.subject;
			body = r.body;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function saveTemplate() {
		const name = prompt('Name this template', subject.trim() || 'New template');
		if (!name?.trim()) return;
		const shared = canShare && confirm('Share it with everyone? (Cancel keeps it just for you.)');
		try {
			const t = await api.post<Template>('/crm/email-templates', { name, subject, body, entity_types: entityType ? [entityType] : [], shared });
			await loadTemplates();
			templateId = t.id;
			toasts.success('Template saved');
		} catch (e) {
			toasts.error(e);
		}
	}
	const chosen = $derived(templates.find((t) => t.id === templateId));
	const canDelete = $derived(!!chosen && (chosen.owner_id === session.user?.id || (chosen.owner_id === null && canShare)));
	async function deleteTemplate() {
		if (!chosen || !confirm(`Delete the template “${chosen.name}”?`)) return;
		try {
			await api.del(`/crm/email-templates/${chosen.id}`);
			templateId = '';
			await loadTemplates();
		} catch (e) {
			toasts.error(e);
		}
	}

	let mailboxes = $state<MailboxInfo[]>([]);
	let suggestions = $state<{ address: string; name: string }[]>([]);
	let docs = $state<{ id: string; number: string; pdf_file_id: string | null; status: string }[]>([]);
	let mailboxId = $state('');
	let to = $state('');
	let cc = $state('');
	let subject = $state('');
	let body = $state('');
	let files = $state<{ id: string; name: string }[]>([]);
	let sending = $state(false);

	onMount(async () => {
		const [m, s, d] = await Promise.all([
			myMailboxes(),
			api.get<{ items: typeof suggestions }>(`/crm/items/${itemId}/email-recipients`).then((r) => r.items).catch(() => []),
			api.get<{ items: typeof docs }>(`/crm/items/${itemId}/documents`).then((r) => r.items.filter((x) => x.status === 'ready' && x.pdf_file_id)).catch(() => [])
		]);
		mailboxes = m;
		suggestions = s;
		loadTemplates();
		docs = d;
		mailboxId = m[0]?.id ?? '';
		if (reply) {
			to = reply.from_addr;
			cc = (reply.cc ?? []).join(', ');
			subject = /^re:/i.test(reply.subject) ? reply.subject : `Re: ${reply.subject}`;
		} else if (s[0]) {
			to = s[0].address;
		}
	});

	const split = (s: string) => s.split(/[,;]/).map((x) => x.trim()).filter(Boolean);

	async function upload(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		for (const file of Array.from(input.files ?? [])) {
			const fd = new FormData();
			fd.append('file', file);
			try {
				const f = await api.post<{ id: string }>('/files', fd);
				files = [...files, { id: f.id, name: file.name }];
			} catch (err) {
				toasts.error(err);
			}
		}
		input.value = '';
	}

	async function send() {
		sending = true;
		try {
			await api.post(`/crm/items/${itemId}/emails`, { mailbox_id: mailboxId, to: split(to), cc: split(cc), subject, body, reply_to: reply?.id, file_ids: files.map((f) => f.id) });
			toasts.success('Email is being sent');
			body = '';
			files = [];
			onsent?.();
		} catch (e) {
			toasts.error(e);
		} finally {
			sending = false;
		}
	}
</script>

{#if !mailboxes.length}
	<p class="text-sm text-slate-500">Connect your mailbox under <a class="text-brand-600 hover:underline" href="/crm/mailboxes">CRM → Mailboxes</a> to write emails from here.</p>
{:else}
	<div class="space-y-2 text-sm">
		<div class="grid grid-cols-[3.5rem_1fr] items-center gap-2">
			<label for="em-from" class="text-slate-500">From</label>
			<select id="em-from" class="input py-1.5" bind:value={mailboxId}>
				{#each mailboxes as m (m.id)}<option value={m.id}>{m.display_name ? `${m.display_name} <${m.email}>` : m.email}</option>{/each}
			</select>
			<label for="em-to" class="text-slate-500">To</label>
			<input id="em-to" class="input py-1.5" list="em-suggest" bind:value={to} placeholder="name@example.com" />
			<label for="em-cc" class="text-slate-500">Cc</label>
			<input id="em-cc" class="input py-1.5" list="em-suggest" bind:value={cc} />
			<datalist id="em-suggest">{#each suggestions as s (s.address)}<option value={s.address}>{s.name}</option>{/each}</datalist>
			<label for="em-subject" class="text-slate-500">Subject</label>
			<input id="em-subject" class="input py-1.5" maxlength="500" bind:value={subject} />
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<select class="input w-auto py-1 text-xs" aria-label="Email template" value={templateId} onchange={(e) => useTemplate(e.currentTarget.value)}>
				<option value="">{templates.length ? 'Use a template…' : 'No templates yet'}</option>
				{#if templates.some((t) => t.owner_id === null)}
					<optgroup label="Shared">{#each templates.filter((t) => t.owner_id === null) as t (t.id)}<option value={t.id}>{t.name}</option>{/each}</optgroup>
				{/if}
				{#if templates.some((t) => t.owner_id !== null)}
					<optgroup label="Mine">{#each templates.filter((t) => t.owner_id !== null) as t (t.id)}<option value={t.id}>{t.name}</option>{/each}</optgroup>
				{/if}
			</select>
			<button type="button" class="btn btn-ghost px-2 py-1 text-xs" disabled={!subject.trim() && !body.trim()} onclick={saveTemplate} title="Placeholders like {'{{title}}'} or {'{{contact}}'} are filled in when the template is used">Save as template</button>
			{#if canDelete}<button type="button" class="btn btn-ghost px-2 py-1 text-xs text-red-600" onclick={deleteTemplate}>Delete template</button>{/if}
		</div>
		<textarea class="input" rows="6" aria-label="Message" placeholder={reply ? 'Your reply (the earlier message is quoted below it)…' : 'Write your message…'} bind:value={body}></textarea>
		<div class="flex flex-wrap items-center gap-2">
			{#each files as f (f.id)}
				<span class="badge flex items-center gap-1 bg-slate-100 text-slate-700"><Icon name="paperclip" class="size-3" />{f.name}<button type="button" aria-label="Remove attachment" onclick={() => (files = files.filter((x) => x.id !== f.id))}><Icon name="x" class="size-3" /></button></span>
			{/each}
			<label class="btn btn-ghost cursor-pointer px-2 py-1 text-xs"><Icon name="paperclip" class="size-3.5" /> Attach<input type="file" multiple class="sr-only" onchange={upload} /></label>
			{#if docs.length}
				<select class="input w-auto py-1 text-xs" aria-label="Attach a document" onchange={(e) => { const d = docs.find((x) => x.id === e.currentTarget.value); if (d && !files.some((f) => f.id === d.pdf_file_id)) files = [...files, { id: d.pdf_file_id!, name: `${d.number}.pdf` }]; e.currentTarget.value = ''; }}>
					<option value="">Attach a document…</option>
					{#each docs as d (d.id)}<option value={d.id}>{d.number}</option>{/each}
				</select>
			{/if}
			<div class="ml-auto flex gap-2">
				{#if oncancel}<button class="btn btn-secondary" onclick={oncancel}>Cancel</button>{/if}
				<button class="btn btn-primary" disabled={sending || !mailboxId || !to.trim() || !subject.trim()} onclick={send}><Icon name="send" class="size-4" /> {sending ? 'Sending…' : 'Send'}</button>
			</div>
		</div>
	</div>
{/if}
