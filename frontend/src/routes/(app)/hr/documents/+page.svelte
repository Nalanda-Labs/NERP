<script lang="ts">
	// HR documents to sign: what waits for me, my documents, and (HR) all of them, templates and
	// sending new ones to employees.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDate } from '#lib/format.ts';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Doc = { id: string; title: string; employee_name: string; status: string; overdue: boolean; due_on: string | null; created_at: string; my_turn: string | null };
	type After = { notify?: string[]; task?: { title: string; responsible: string }; list_id?: string };
	type Tpl = { id: string; name: string; body: string; after_signing?: After };
	type Person = { id: string; name: string; avatar_file_id: string | null };
	const hr = $derived(session.can(PERM.HR));
	let tab = $state<'todo' | 'mine' | 'all' | 'templates'>('todo');
	let items = $state<Doc[]>([]);
	let templates = $state<Tpl[]>([]);
	const STATUS: Record<string, string> = { approval: 'Being approved', signing: 'Being signed', signed: 'Signed', declined: 'Declined', revoked: 'Not valid' };

	async function load() {
		try {
			if (tab === 'templates') templates = (await api.get<{ items: Tpl[] }>('/hr/doc-templates')).items;
			else items = (await api.get<{ items: Doc[] }>(`/hr/docs?scope=${tab}`)).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);

	// Sending documents.
	let sendOpen = $state(false);
	let form = $state({ title: '', template_id: '', due_on: '' });
	let employees = $state<Person[]>([]);
	let approvers = $state<Person[]>([]);
	let signer = $state<string | null>(null);
	let signerLabel = $state<string | null>(null);
	let file = $state<File | null>(null);
	let busy = $state(false);
	async function openSend() {
		templates = (await api.get<{ items: Tpl[] }>('/hr/doc-templates')).items;
		form = { title: '', template_id: templates[0]?.id ?? '', due_on: '' };
		employees = [];
		approvers = [];
		signer = null;
		file = null;
		sendOpen = true;
	}
	async function send(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const body: Record<string, unknown> = { title: form.title, employee_ids: employees.map((p) => p.id), approver_ids: approvers.map((p) => p.id), company_signer_id: signer, due_on: form.due_on || null };
			if (form.template_id === 'file') {
				if (!file) throw new Error('choose a file');
				const fd = new FormData();
				fd.append('file', file);
				body.file_id = (await api.post<{ id: string }>('/files', fd)).id;
			} else body.template_id = form.template_id;
			const r = await api.post<{ ids: string[] }>('/hr/docs', body);
			sendOpen = false;
			toasts.success(`${r.ids.length} document${r.ids.length === 1 ? '' : 's'} sent`);
			tab = 'all';
			load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	// Templates.
	type TplForm = { id: string | null; name: string; body: string; notify: PickedUser[]; taskTitle: string; taskTo: string; listId: string };
	let tplForm = $state<TplForm | null>(null);
	let addableLists = $state<{ id: string; name: string }[]>([]);
	async function openTpl(t: Tpl | null, fresh = '') {
		const a = t?.after_signing ?? {};
		const notify: PickedUser[] = [];
		for (const id of a.notify ?? []) {
			const u = await api.get<{ id: string; first_name: string; last_name: string; avatar_file_id: string | null }>(`/users/${id}`).catch(() => null);
			if (u) notify.push({ id: u.id, name: `${u.first_name} ${u.last_name}`, avatar_file_id: u.avatar_file_id });
		}
		addableLists = ((await api.get<{ items: { id: string; name: string; level: string }[] }>('/lists').catch(() => ({ items: [] }))).items).filter((l) => l.level === 'add' || l.level === 'manage');
		tplForm = { id: t?.id ?? null, name: t?.name ?? '', body: t?.body ?? fresh, notify, taskTitle: a.task?.title ?? '', taskTo: a.task?.responsible ?? 'hr', listId: a.list_id ?? '' };
	}
	async function saveTpl(e: SubmitEvent) {
		e.preventDefault();
		try {
			const t = tplForm!;
			const after_signing: After = {};
			if (t.notify.length) after_signing.notify = t.notify.map((u) => u.id);
			if (t.taskTitle.trim()) after_signing.task = { title: t.taskTitle, responsible: t.taskTo };
			if (t.listId) after_signing.list_id = t.listId;
			const body = { name: t.name, body: t.body, after_signing };
			templates = (t.id ? await api.patch<{ items: Tpl[] }>(`/hr/doc-templates/${t.id}`, body) : await api.post<{ items: Tpl[] }>('/hr/doc-templates', body)).items;
			tplForm = null;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function dropTpl(t: Tpl) {
		if (!confirm(`Delete the template “${t.name}”?`)) return;
		templates = (await api.del<{ items: Tpl[] }>(`/hr/doc-templates/${t.id}`)).items;
	}
</script>

<svelte:head><title>Documents to sign · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-2">
		<h1 class="mr-2 text-2xl font-semibold text-slate-900">Documents to sign</h1>
		<div class="flex rounded-full bg-slate-100 p-0.5" role="tablist">
			{#each [['todo', 'For me to do'], ['mine', 'Mine'], ...(hr ? [['all', 'All'], ['templates', 'Templates']] : [])] as [v, l] (v)}
				<button role="tab" aria-selected={tab === v} class="rounded-full px-3 py-1 text-sm font-medium {tab === v ? 'bg-white shadow-sm' : 'text-slate-600'}" onclick={() => ((tab = v as typeof tab), load())}>{l}</button>
			{/each}
		</div>
		{#if hr}<button class="btn btn-primary ml-auto" onclick={openSend}>Send documents</button>{/if}
	</div>

	{#if tab === 'templates'}
		<div class="space-y-2">
			{#each templates as t (t.id)}
				<div class="card flex items-start gap-3 p-4" data-template={t.name}><div class="min-w-0 flex-1"><p class="font-medium">{t.name}</p><p class="truncate text-sm text-slate-500">{t.body.slice(0, 160)}</p></div><button class="btn btn-ghost px-2 py-1" onclick={() => openTpl(t)}>Edit</button><button class="btn btn-ghost px-2 py-1 hover:text-red-600" onclick={() => dropTpl(t)}>Delete</button></div>
			{:else}<p class="text-sm text-slate-500">No templates yet.</p>{/each}
			<button class="btn btn-secondary" onclick={() => openTpl(null, '# Title\n\n{{company}} and {{name}} ({{position}}) agree that …\n\nDate: {{date}}')}>New template</button>
		</div>
	{:else}
		<ul class="card divide-y divide-slate-100">
			{#each items as d (d.id)}
				<li><a class="flex flex-wrap items-center gap-3 px-5 py-3 hover:bg-slate-50" href="/hr/documents/{d.id}" data-doc={d.title}>
					<span class="font-medium text-slate-900">{d.title}</span><span class="text-sm text-slate-500">{d.employee_name}</span>
					{#if d.my_turn}<span class="badge bg-amber-50 text-amber-800">{d.my_turn === 'approver' ? 'To approve' : 'To sign'}</span>{/if}
					<span class="ml-auto text-sm {d.overdue ? 'text-red-600' : 'text-slate-500'}">{STATUS[d.status]}{d.due_on && ['approval', 'signing'].includes(d.status) ? ` · due ${formatDate(d.due_on)}` : ''}</span>
				</a></li>
			{:else}<li class="px-5 py-8 text-center text-sm text-slate-400">{tab === 'todo' ? 'Nothing waiting for you.' : 'No documents.'}</li>{/each}
		</ul>
	{/if}
</div>

<Modal bind:open={sendOpen} title="Send documents to sign">
	<form id="send-form" class="space-y-3 text-sm" onsubmit={send}>
		<div><label class="label" for="sd-title">Title</label><input id="sd-title" class="input" required maxlength="300" bind:value={form.title} /></div>
		<div><label class="label" for="sd-src">From</label><select id="sd-src" class="input" bind:value={form.template_id}>{#each templates as t (t.id)}<option value={t.id}>Template: {t.name}</option>{/each}<option value="file">A file (PDF, Word, OpenDocument)</option></select></div>
		{#if form.template_id === 'file'}<input type="file" class="input" aria-label="File" accept=".pdf,.doc,.docx,.odt,.rtf" onchange={(e) => (file = e.currentTarget.files?.[0] ?? null)} />{/if}
		<div><span class="label">Employees <span class="font-normal text-slate-400">(each gets their own copy)</span></span><MultiUserPicker bind:value={employees} placeholder="Add employees…" /></div>
		<div><span class="label">Approved first by <span class="font-normal text-slate-400">(optional, in this order)</span></span><MultiUserPicker bind:value={approvers} placeholder="Add approvers…" /></div>
		<div><span class="label">Signs for the company <span class="font-normal text-slate-400">(optional)</span></span><UserPicker bind:value={signer} bind:label={signerLabel} placeholder="Company signer…" /></div>
		<div><label class="label" for="sd-due">Sign by <span class="font-normal text-slate-400">(optional)</span></label><input id="sd-due" class="input w-48" type="date" bind:value={form.due_on} /></div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (sendOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="send-form" disabled={busy || !employees.length}>Send</button>
	{/snippet}
</Modal>

<Modal open={!!tplForm} title={tplForm?.id ? 'Edit template' : 'New template'} onclose={() => (tplForm = null)}>
	{#if tplForm}
		<form id="tpl-form" class="space-y-3 text-sm" onsubmit={saveTpl}>
			<div><label class="label" for="tp-name">Name</label><input id="tp-name" class="input" required maxlength="200" bind:value={tplForm.name} /></div>
			<div><label class="label" for="tp-body">Text</label><textarea id="tp-body" class="input font-mono" rows="12" required bind:value={tplForm.body}></textarea>
				<p class="mt-1 text-xs text-slate-500">Paragraphs are separated by a blank line; a line starting with “# ” is a heading. Fields: {'{{name}}'}, {'{{first_name}}'}, {'{{last_name}}'}, {'{{position}}'}, {'{{department}}'}, {'{{email}}'}, {'{{date}}'}, {'{{company}}'}.</p></div>
			<fieldset class="space-y-3 rounded-xl border border-slate-200 p-3" data-after-signing>
				<legend class="px-1 text-sm font-medium text-slate-700">When everyone has signed</legend>
				<div><span class="label">Tell <span class="font-normal text-slate-400">(optional)</span></span><MultiUserPicker bind:value={tplForm.notify} placeholder="Add people…" /></div>
				<div class="grid gap-2 sm:grid-cols-[1fr_12rem]">
					<div><label class="label" for="tp-task">Make a task <span class="font-normal text-slate-400">(optional; {'{{name}}'} and {'{{document}}'} work)</span></label><input id="tp-task" class="input" maxlength="300" bind:value={tplForm.taskTitle} placeholder="e.g. Order a laptop for {'{{name}}'}" /></div>
					<div><label class="label" for="tp-taskto">For</label><select id="tp-taskto" class="input" bind:value={tplForm.taskTo}><option value="hr">Whoever sent it</option><option value="employee">The employee</option></select></div>
				</div>
				<div>
					<label class="label" for="tp-list">Add an item to a list <span class="font-normal text-slate-400">(starts its processes)</span></label>
					<select id="tp-list" class="input" bind:value={tplForm.listId}><option value="">—</option>{#each addableLists as l (l.id)}<option value={l.id}>{l.name}</option>{/each}</select>
					<p class="mt-1 text-xs text-slate-500">The item gets the employee in its first person field, today in its first date field and the document's title in its first text field.</p>
				</div>
			</fieldset>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (tplForm = null)}>Cancel</button>
		<button class="btn btn-primary" form="tpl-form">Save</button>
	{/snippet}
</Modal>
