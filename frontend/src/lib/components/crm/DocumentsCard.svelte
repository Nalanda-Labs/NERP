<script lang="ts">
	// Quotes, invoices and other PDFs generated from templates for one CRM record.
	// LuaLaTeX and LibreOffice documents render in the background; their status arrives
	// over the real-time connection.
	import { onDestroy, onMount } from 'svelte';
	import { api, downloadFile, openFile } from '#lib/api.ts';
	import { formatRelative } from '#lib/format.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';

	type Template = { id: string; name: string; engine: string; doc_title: string };
	type Doc = {
		id: string;
		template_id: string | null;
		template_name: string;
		engine: string;
		number: string;
		status: 'pending' | 'ready' | 'failed';
		pdf_file_id: string | null;
		source_file_id: string | null;
		error: string | null;
		created_by_name: string | null;
		created_at: string;
		updated_at: string;
	};

	let { itemId, typeCode, canWrite, hasBuyer = false }: { itemId: string; typeCode: string; canWrite: boolean; hasBuyer?: boolean } = $props();

	let templates = $state<Template[]>([]);
	let docs = $state<Doc[]>([]);
	let menu = $state(false);
	let busy = $state(false);
	let shownError = $state<string | null>(null);

	// Deals and leads: which requisites (of the company or a contact) documents are billed to.
	type BuyerOption = { id: string; item_title: string; name: string; country: string; tax_line: string; is_default: boolean };
	let buyer = $state<{ chosen: string | null; current: string | null; options: BuyerOption[] } | null>(null);
	async function loadBuyer() {
		if (!hasBuyer) return;
		try {
			buyer = await api.get(`/crm/items/${itemId}/buyer`);
		} catch {
			buyer = null; // the record was deleted or is no longer visible
		}
	}
	async function setBuyer(id: string) {
		try {
			buyer = await api.put(`/crm/items/${itemId}/buyer`, { requisite_id: id || null });
		} catch (e) {
			toasts.error(e);
		}
	}

	async function load() {
		docs = (await api.get<{ items: Doc[] }>(`/crm/items/${itemId}/documents`)).items;
		loadSignatures();
	}

	// E-signature: the latest request per document.
	type Signer = { id: string; name: string; email: string; status: 'pending' | 'viewed' | 'signed' | 'declined'; signed_at: string | null; decline_reason: string | null };
	type SignRequest = { id: string; document_id: string | null; title: string; status: 'sent' | 'completed' | 'declined' | 'cancelled' | 'expired'; signed_file_id: string | null; created_at: string; signers: Signer[] };
	let requests = $state<SignRequest[]>([]);
	const latest = (docId: string) => requests.find((r) => r.document_id === docId);
	async function loadSignatures() {
		try {
			requests = (await api.get<{ items: SignRequest[] }>(`/crm/items/${itemId}/signatures`)).items;
		} catch {
			requests = [];
		}
	}
	let signDoc = $state<Doc | null>(null);
	let signOpen = $state(false);
	let signers = $state<{ name: string; email: string }[]>([]);
	let signMessage = $state('');
	let openRequest = $state<string | null>(null);
	// Signing templates: who signs, by role, filled in for this record.
	type SignTemplate = { id: string; name: string; message: string | null };
	let signTemplates = $state<SignTemplate[]>([]);
	let templateId = $state('');
	let templateError = $state('');
	async function startSigning(d: Doc) {
		signDoc = d;
		signers = [{ name: '', email: '' }];
		signMessage = '';
		templateId = '';
		templateError = '';
		signOpen = true;
		signTemplates = (await api.get<{ items: SignTemplate[] }>('/crm/sign-templates').catch(() => ({ items: [] }))).items;
	}
	async function useTemplate() {
		templateError = '';
		if (!templateId) return;
		const t = signTemplates.find((x) => x.id === templateId);
		try {
			signers = (await api.get<{ signers: { name: string; email: string }[] }>(`/crm/items/${itemId}/sign-templates/${templateId}/signers`)).signers;
			signMessage = t?.message ?? '';
		} catch (e) {
			templateError = e instanceof Error ? e.message : 'This template can\'t be used here';
			templateId = '';
		}
	}
	async function sendForSignature(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			if (templateId) await api.post(`/crm/items/${itemId}/sign`, { template_id: templateId, document_id: signDoc!.id });
			else await api.post(`/crm/documents/${signDoc!.id}/signature`, { signers, message: signMessage || null });
			signOpen = false;
			toasts.success('Sent for signature');
			await loadSignatures();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function requestAction(r: SignRequest, action: 'remind' | 'cancel') {
		if (action === 'cancel' && !confirm(`Cancel the signature request for ${r.title}? Signing links stop working.`)) return;
		try {
			await api.post(`/crm/signatures/${r.id}/${action}`, {});
			toasts.success(action === 'remind' ? 'Links sent again' : 'Cancelled');
			await loadSignatures();
		} catch (err) {
			toasts.error(err);
		}
	}
	const SIGN_STATUS: Record<string, [string, string]> = {
		sent: ['Waiting for signatures', 'bg-amber-50 text-amber-800'],
		completed: ['Signed', 'bg-emerald-50 text-emerald-800'],
		declined: ['Declined', 'bg-red-50 text-red-700'],
		cancelled: ['Signing cancelled', 'bg-slate-100 text-slate-500'],
		expired: ['Signing expired', 'bg-slate-100 text-slate-500']
	};
	const SIGNER_STATUS: Record<string, string> = { pending: 'not opened yet', viewed: 'opened', signed: 'signed', declined: 'declined' };
	onMount(async () => {
		await Promise.all([load(), loadBuyer()]);
		templates = (await api.get<{ items: Template[] }>(`/crm/doc-templates?type=${typeCode}`)).items;
	});
	// The company or contacts may change, and with them the available requisites.
	const offChanged = realtime.on('crm.changed', (e: { id: string; deleted: boolean }) => {
		if (e.id === itemId && !e.deleted) loadBuyer();
	});
	onDestroy(offChanged);
	const off = realtime.on('crm.document', (e: { item_id: string }) => {
		if (e.item_id === itemId) load();
	});
	onDestroy(off);

	async function create(t: Template) {
		menu = false;
		busy = true;
		try {
			const d = await api.post<Doc>(`/crm/items/${itemId}/documents`, { template_id: t.id });
			await load();
			if (d.status === 'ready' && d.pdf_file_id) await openFile(d.pdf_file_id);
			else if (d.status === 'failed') toasts.error(new Error(d.error ?? 'The document could not be created'));
			else toasts.success(`${d.number} is being prepared…`);
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}

	async function regenerate(d: Doc) {
		try {
			await api.post(`/crm/documents/${d.id}/regenerate`);
			await load();
			toasts.success(`${d.number} updated with the current data`);
		} catch (e) {
			toasts.error(e);
		}
	}

	async function remove(d: Doc) {
		if (!confirm(`Delete ${d.number}? Its number is not reused.`)) return;
		try {
			await api.del(`/crm/documents/${d.id}`);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	const ENGINE: Record<string, string> = { typst: 'Typst', latex: 'LaTeX', office: 'Word' };
</script>

<section class="card">
	<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3">
		<h2 class="font-medium text-slate-900">Documents</h2>
		{#if templates.length}
			<div class="relative">
				<button class="btn btn-ghost text-sm" disabled={busy} onclick={() => (menu = !menu)} aria-haspopup="menu" aria-expanded={menu}>
					<Icon name="plus" class="size-4" /> {busy ? 'Creating…' : 'Create document'}
				</button>
				{#if menu}
					<ul class="absolute right-0 z-20 mt-1 w-64 rounded-xl bg-white py-1 shadow-g2" role="menu">
						{#each templates as t (t.id)}
							<li>
								<button class="flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-sm hover:bg-slate-50" role="menuitem" onclick={() => create(t)}>
									<span class="truncate">{t.name}</span><span class="text-xs text-slate-400">{ENGINE[t.engine]}</span>
								</button>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		{/if}
	</div>
	{#if hasBuyer && buyer?.options.length}
		<div class="flex items-center gap-2 border-b border-slate-50 px-5 py-2.5 text-sm">
			<label for="doc-buyer" class="shrink-0 text-slate-500">Bill to</label>
			<select id="doc-buyer" class="input min-w-0 flex-1 py-1.5 text-sm" value={buyer.chosen ?? ''} disabled={!canWrite} onchange={(e) => setBuyer(e.currentTarget.value)}>
				<option value="">Automatic — {buyer.options.find((o) => o.id === buyer!.current)?.name ?? 'none'}</option>
				{#each buyer.options as o (o.id)}<option value={o.id}>{o.name} ({o.item_title}){o.tax_line ? ` · ${o.tax_line}` : ''}</option>{/each}
			</select>
		</div>
	{/if}
	<ul class="divide-y divide-slate-50">
		{#each docs as d (d.id)}
			<li class="flex items-center gap-3 px-5 py-2.5 text-sm">
				<Icon name="file" class="size-4 shrink-0 {d.status === 'failed' ? 'text-red-500' : 'text-slate-400'}" />
				<div class="min-w-0 flex-1">
					{#if d.status === 'ready' && d.pdf_file_id}
						<button class="font-medium text-slate-800 hover:text-brand-700" onclick={() => openFile(d.pdf_file_id!).catch(toasts.error)}>{d.number}</button>
					{:else}
						<span class="font-medium text-slate-800">{d.number}</span>
					{/if}
					<p class="truncate text-xs text-slate-500">
						{d.template_name}{d.created_by_name ? ` · ${d.created_by_name}` : ''} · {formatRelative(d.created_at)}
					</p>
				</div>
				{#if d.status === 'pending'}
					<span class="badge bg-amber-50 text-amber-700">Preparing…</span>
				{:else if d.status === 'failed'}
					<button class="badge bg-red-50 text-red-700" onclick={() => (shownError = shownError === d.id ? null : d.id)}>Failed</button>
				{:else}
					<button class="text-slate-400 hover:text-slate-800" title="Download PDF" aria-label="Download PDF" onclick={() => downloadFile(d.pdf_file_id!, `${d.number}.pdf`).catch(toasts.error)}><Icon name="download" class="size-4" /></button>
					{#if d.source_file_id}
						<button class="text-xs font-medium text-slate-400 hover:text-slate-800" title="Download the filled Word file" onclick={() => downloadFile(d.source_file_id!, `${d.number}.docx`).catch(toasts.error)}>DOCX</button>
					{/if}
				{/if}
				{#if d.status === 'ready' && d.pdf_file_id}
					{@const r = latest(d.id)}
					{#if r}
						<button class="badge {SIGN_STATUS[r.status][1]}" data-sign-status={r.status} onclick={() => (openRequest = openRequest === r.id ? null : r.id)}>
							{r.status === 'sent' ? `${r.signers.filter((x) => x.status === 'signed').length} of ${r.signers.length} signed` : SIGN_STATUS[r.status][0]}
						</button>
					{/if}
					{#if canWrite && r?.status !== 'sent' && r?.status !== 'completed'}
						<button class="text-xs font-medium text-brand-600 hover:underline" onclick={() => startSigning(d)}>Send for signature</button>
					{/if}
				{/if}
				{#if d.template_id && d.status !== 'pending'}
					<button class="text-slate-400 hover:text-slate-800" title="Regenerate with current data" aria-label="Regenerate" onclick={() => regenerate(d)}><Icon name="refresh" class="size-4" /></button>
				{/if}
				{#if canWrite}
					<button class="text-slate-300 hover:text-red-600" title="Delete" aria-label="Delete document" onclick={() => remove(d)}><Icon name="trash" class="size-4" /></button>
				{/if}
			</li>
			{@const req = latest(d.id)}
			{#if req && openRequest === req.id}
				<li class="space-y-2 bg-slate-50/60 px-5 py-3 text-sm" data-sign-request>
					<ul class="space-y-1">
						{#each req.signers as x (x.id)}
							<li class="flex items-center gap-2">
								<span class="font-medium text-slate-800">{x.name}</span><span class="text-xs text-slate-500">{x.email}</span>
								<span class="ml-auto text-xs {x.status === 'signed' ? 'text-emerald-700' : x.status === 'declined' ? 'text-red-700' : 'text-slate-500'}">{SIGNER_STATUS[x.status]}{x.signed_at ? ` ${formatRelative(x.signed_at)}` : ''}</span>
							</li>
							{#if x.decline_reason}<li class="pl-4 text-xs text-red-700">“{x.decline_reason}”</li>{/if}
						{/each}
					</ul>
					<div class="flex gap-3">
						{#if req.signed_file_id}<button class="text-xs font-medium text-brand-600 hover:underline" onclick={() => downloadFile(req.signed_file_id!, `${req.title} (signed).pdf`).catch(toasts.error)}>Download the signed copy</button>{/if}
						{#if req.status === 'sent' && canWrite}
							<button class="text-xs font-medium text-brand-600 hover:underline" onclick={() => requestAction(req, 'remind')}>Send the links again</button>
							<button class="text-xs font-medium text-slate-500 hover:text-red-600" onclick={() => requestAction(req, 'cancel')}>Cancel request</button>
						{/if}
					</div>
				</li>
			{/if}
			{#if shownError === d.id && d.error}
				<li class="bg-red-50/50 px-5 py-2"><pre class="max-h-48 overflow-auto text-xs whitespace-pre-wrap text-red-800">{d.error}</pre></li>
			{/if}
		{:else}
			<li class="px-5 py-3 text-sm text-slate-400">{templates.length ? 'No documents yet.' : 'No document templates for this record type.'}</li>
		{/each}
	</ul>
</section>

<Modal bind:open={signOpen} title={signDoc ? `Send ${signDoc.number} for signature` : ''}>
	<form id="sign-form" class="space-y-3" onsubmit={sendForSignature}>
		<p class="text-sm text-slate-500">Each signer gets a personal link by email to read the PDF and sign it. When everyone has signed, they all get the signed copy with a certificate page, and it appears here.</p>
		{#if signTemplates.length}
			<div>
				<label class="label" for="sign-tpl">From a template</label>
				<select id="sign-tpl" class="input" bind:value={templateId} onchange={useTemplate}>
					<option value="">— Choose the signers yourself —</option>
					{#each signTemplates as t (t.id)}<option value={t.id}>{t.name}</option>{/each}
				</select>
				{#if templateError}<p class="mt-1 text-xs text-red-600" role="alert">{templateError}</p>{/if}
			</div>
		{/if}
		{#each signers as x, i (i)}
			<div class="flex gap-2">
				<input class="input flex-1" required maxlength="200" placeholder="Name" aria-label="Signer {i + 1} name" bind:value={x.name} readonly={!!templateId} />
				<input class="input flex-1" type="email" required placeholder="Email" aria-label="Signer {i + 1} email" bind:value={x.email} readonly={!!templateId} />
				{#if signers.length > 1}<button type="button" class="btn btn-ghost p-1.5" aria-label="Remove signer" onclick={() => signers.splice(i, 1)}><Icon name="x" class="size-4" /></button>{/if}
			</div>
		{/each}
		{#if templateId}
			<button type="button" class="text-sm font-medium text-brand-600 hover:underline" onclick={() => (templateId = '')}>Change the signers</button>
		{:else if signers.length < 10}<button type="button" class="text-sm font-medium text-brand-600 hover:underline" onclick={() => signers.push({ name: '', email: '' })}>+ Add signer</button>{/if}
		<div><label class="label" for="sign-msg">Message <span class="font-normal text-slate-400">(optional)</span></label><textarea id="sign-msg" class="input" rows="2" maxlength="2000" bind:value={signMessage}></textarea></div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (signOpen = false)}>Cancel</button>
		<button class="btn btn-primary" form="sign-form" disabled={busy}>Send</button>
	{/snippet}
</Modal>
