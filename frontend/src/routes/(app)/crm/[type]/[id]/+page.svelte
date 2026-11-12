<script lang="ts">
	import ChatCallButtons from '#lib/components/ChatCallButtons.svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, downloadFile } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import CrmRecordForm from '#lib/components/crm/CrmRecordForm.svelte';
	import CardSections from '#lib/components/crm/CardSections.svelte';
	import ProductRows from '#lib/components/crm/ProductRows.svelte';
	import DocumentsCard from '#lib/components/crm/DocumentsCard.svelte';
	import BookingsCard from '#lib/components/crm/BookingsCard.svelte';
	import DealHealth from '#lib/components/crm/DealHealth.svelte';
	import PromiseCheck from '#lib/components/crm/PromiseCheck.svelte';
	import ClientEverything from '#lib/components/crm/ClientEverything.svelte';
	import RequisitesCard from '#lib/components/crm/RequisitesCard.svelte';
	import RecurringCard from '#lib/components/crm/RecurringCard.svelte';
	import InvoiceBooksCard from '#lib/components/accounting/InvoiceBooksCard.svelte';
	import EmailComposer, { type ReplyTarget } from '#lib/components/crm/EmailComposer.svelte';
	import EmailView from '#lib/components/crm/EmailView.svelte';
	import Icon from '#lib/components/Icon.svelte';
	import BookingLink from '#lib/components/crm/BookingLink.svelte';
	import Journey from '#lib/components/crm/Journey.svelte';
	import RecordProcesses from '#lib/components/crm/RecordProcesses.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import { cardLayout, crm, durationText, formatAddress, invoiceOverdue, itemLink, optionLabel, ufText, money, overLimit, type Activity, type CardLayoutInfo, type CrmDetail, type CrmType, typeName } from '#lib/crm.svelte.ts';
	import { formatBytes, formatDateTime, formatRelative } from '#lib/format.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { PERM } from '#lib/types.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Entry = { id: string; kind: string; author_id: string | null; author_name: string | null; author_avatar_file_id: string | null; body: string | null; data: Record<string, any>; is_pinned: boolean; files: { id: string; name: string; size: number; mime: string }[]; created_at: string };

	const code = $derived(page.params.type!);
	const id = $derived(page.params.id!);
	// The record's chat: its responsible person, creator and observers (opening it makes you one).
	const recordChat = async () => (await api.post<{ chat_id: string }>(`/crm/items/${id}/chat`)).chat_id;
	const isNew = $derived(id === 'new');
	let type = $state<CrmType | null>(null);
	let item = $state<CrmDetail | null>(null);
	let notFound = $state(false);
	let editing = $state(false);
	let timeline = $state<{ pinned: Entry[]; items: Entry[]; has_more: boolean }>({ pinned: [], items: [], has_more: false });
	let activities = $state<Activity[]>([]);
	type Run = { id: string; robot_name: string; run_at: string; status: string; wait_until: string | null; is_test?: boolean };
	let waiting = $state<Run[]>([]);

	let cardInfo = $state<CardLayoutInfo | null>(null);

	// ----- tasks about this record -----
	type TaskRow = { id: string; number: number; title: string; status: string; deadline: string | null; is_overdue: boolean; responsible_name: string; responsible_avatar_file_id: string | null };
	let tasks = $state<TaskRow[]>([]);
	let taskOpen = $state(false);
	let newTask = $state({ title: '', deadline: '', responsible_id: '', responsible_name: '' });
	function openTask() {
		newTask = { title: item ? `Follow up: ${item.title}` : '', deadline: '', responsible_id: session.user!.id, responsible_name: session.user!.full_name };
		taskOpen = true;
	}
	async function addTask(e: SubmitEvent) {
		e.preventDefault();
		if (!item) return;
		try {
			await api.post('/tasks', {
				title: newTask.title,
				deadline: newTask.deadline ? new Date(newTask.deadline).toISOString() : null,
				responsible_id: newTask.responsible_id || null,
				crm_item_ids: [item.id]
			});
			taskOpen = false;
			toasts.success('Task created');
			load();
		} catch (err) {
			toasts.error(err);
		}
	}

	// ----- observers -----
	async function setObservers(ids: string[]) {
		if (!item) return;
		try {
			const r = await api.put<{ observers: CrmDetail['observers'] }>(`/crm/items/${item.id}/observers`, { user_ids: ids });
			item.observers = r.observers;
		} catch (err) {
			toasts.error(err);
		}
	}

	// ----- edit history -----
	const HISTORY_LABELS: Record<string, string> = { company_id: 'Company', category_id: 'Pipeline', multifields: 'Contact details', contacts: 'Contacts', title: 'Name', address: 'Address', comments: 'Comment', currency: 'Currency', opportunity: 'Amount' };
	function historyLabel(code: string): string {
		if (code.startsWith('uf.')) return type?.custom_fields.find((d) => `uf.${d.code}` === code)?.label ?? code.slice(3);
		return type?.system_fields.find((f) => f.code === code)?.label ?? HISTORY_LABELS[code] ?? code;
	}
	function historyValue(code: string, v: unknown): string {
		if (v === null || v === undefined || v === '') return '—';
		if (code === 'address') return formatAddress(v as never) || '—';
		if (code === 'close_date' || code === 'birthdate') return new Date(String(v) + 'T00:00').toLocaleDateString();
		if (['source', 'company_kind', 'industry'].includes(code)) return optionLabel(String(v));
		if (code === 'opportunity' || code === 'annual_revenue') return Number(v).toLocaleString();
		if (code.startsWith('uf.')) {
			const d = type?.custom_fields.find((x) => `uf.${x.code}` === code);
			return (d && ufText(d, v)) || String(v);
		}
		return String(v);
	}
	async function load() {
		await crm.load();
		type = crm.type(code) ?? null;
		if (type) cardLayout(code, true).then((i) => (cardInfo = i)).catch(() => {});
		if (!type || isNew) return;
		try {
			item = await api.get<CrmDetail>(`/crm/${code}/items/${id}`);
			notFound = false;
			const [t, a] = await Promise.all([
				api.get<typeof timeline>(`/crm/items/${id}/timeline`),
				api.get<{ items: Activity[] }>(`/crm/activities?item_id=${id}&state=open`)
			]);
			timeline = t;
			activities = a.items;
			api.get<{ items: TaskRow[] }>(`/crm/items/${id}/tasks`).then((r) => (tasks = r.items)).catch(() => (tasks = []));
			shopOrder = null;
			if (type.kind === 'order') api.get<ShopOrder>(`/shop/orders/${id}`).then((o) => (shopOrder = o)).catch(() => {});
			waiting = (await api.get<{ items: Run[] }>(`/crm/items/${id}/automation`)).items
				.filter((r) => r.status === 'waiting' || r.status === 'pending' || (r.status === 'scheduled' && new Date(r.run_at).getTime() > Date.now() + 5000))
				.reverse();
		} catch {
			notFound = true;
		}
	}
	$effect(() => {
		id;
		item = null;
		editing = false;
		load();
	});
	onMount(() => {
		let t: ReturnType<typeof setTimeout>;
		const offChanged = realtime.on('crm.changed', (d: { id: string; deleted: boolean }) => {
			if (d.id !== id || editing) return;
			if (d.deleted) return goto(`/crm/${code}`);
			clearTimeout(t);
			t = setTimeout(load, 300);
		});
		// Documents add timeline entries without changing the record itself.
		const offDocs = realtime.on('crm.document', async (d: { item_id: string }) => {
			if (d.item_id === id) timeline = await api.get<typeof timeline>(`/crm/items/${id}/timeline`);
		});
		return () => (offChanged(), offDocs());
	});

	const defaults = $derived.by(() => {
		const p = page.url.searchParams;
		return {
			category_id: p.get('category_id') ?? undefined,
			company: p.get('company_id') ? { id: p.get('company_id')!, title: p.get('company_title') ?? '' } : undefined,
			contacts: p.get('contact_id') ? [{ id: p.get('contact_id')!, title: p.get('contact_title') ?? '' }] : undefined
		};
	});

	const category = $derived(type?.categories.find((c) => c.id === item?.category_id));
	const openStages = $derived(category?.stages.filter((s) => s.semantics === 'process') ?? []);
	const currentIdx = $derived(openStages.findIndex((s) => s.id === item?.stage_id));
	const currentStage = $derived(category?.stages.find((s) => s.id === item?.stage_id));
	const late = $derived(item ? overLimit(currentStage, item.stage_changed_at) : null);

	// What the customer entered at checkout, for orders from the online shop.
	type ShopOrder = { name: string; email: string; phone: string | null; address: string; note: string | null; stock_returned: boolean; order_url: string };
	let shopOrder = $state<ShopOrder | null>(null);

	// Quotes and invoices made from this record.
	const quoteType = $derived(crm.config?.types.find((t) => t.kind === 'quote'));
	const invoiceType = $derived(crm.config?.types.find((t) => t.kind === 'invoice'));
	const makeTargets = $derived(
		[code === 'deal' ? quoteType : undefined, code === 'deal' || type?.kind === 'quote' || type?.kind === 'order' ? invoiceType : undefined].filter(
			(t): t is CrmType => !!t && t.rights.add !== 'none'
		)
	);
	let making = $state(false);
	async function makeFrom(target: CrmType) {
		if (!item) return;
		making = true;
		try {
			const d = await api.post<CrmDetail>(`/crm/${target.code}/items/from/${item.id}`, {});
			toasts.success(`${target.name} #${d.number} created`);
			goto(`/crm/${target.code}/${d.id}`);
		} catch (e) {
			toasts.error(e);
		} finally {
			making = false;
		}
	}
	/** Invoiced and paid totals over this record's invoices in its own currency. */
	const billing = $derived.by(() => {
		if (!item || !invoiceType) return null;
		const inv = item.children.filter((c) => c.entity_type === invoiceType.id && c.currency === item!.currency && c.stage_semantics !== 'failure');
		if (!inv.length) return null;
		const sum = (l: typeof inv) => l.reduce((a, c) => a + Number(c.opportunity), 0);
		return { invoiced: sum(inv), paid: sum(inv.filter((c) => c.stage_semantics === 'success')) };
	});

	async function setStage(stageId: string) {
		try {
			item = await api.post<CrmDetail>(`/crm/${code}/items/${id}/move`, { stage_id: stageId });
			load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function remove() {
		if (!confirm(`Move "${item!.title}" to the recycle bin?`)) return;
		await api.del(`/crm/${code}/items/${id}`);
		toasts.success('Moved to the recycle bin');
		goto(`/crm/${code}`);
	}

	// ----- composer -----
	let mode = $state<'comment' | 'todo' | 'call' | 'meeting' | 'email'>('comment');
	let replyTo = $state<ReplyTarget | null>(null);
	let openEmails = $state<Record<string, boolean>>({});
	let composerKey = $state(0);
	function startReply(r: ReplyTarget) {
		replyTo = r;
		mode = 'email';
		composerKey++;
		window.scrollTo({ top: 0, behavior: 'smooth' });
	}
	let text = $state('');
	let when = $state('');
	let direction = $state<'outgoing' | 'incoming'>('outgoing');
	let resp = $state<string | null>(null);
	let respLabel = $state<string | null>(null);
	let posting = $state(false);
	// Files for the comment: uploaded as they are chosen, attached when it is posted.
	let attached = $state<{ id: string; name: string }[]>([]);
	let uploading = $state(false);
	async function attach(e: Event) {
		const input = e.target as HTMLInputElement;
		uploading = true;
		try {
			for (const f of input.files ?? []) {
				const fd = new FormData();
				fd.append('file', f);
				attached.push({ id: (await api.post<{ id: string }>('/files', fd)).id, name: f.name });
			}
		} catch (err) {
			toasts.error(err);
		} finally {
			uploading = false;
			input.value = '';
		}
	}
	async function post() {
		if (!text.trim() && !(mode === 'comment' && attached.length)) return;
		posting = true;
		try {
			if (mode === 'comment') {
				await api.post(`/crm/items/${id}/timeline`, { body: text, file_ids: attached.map((f) => f.id) });
				attached = [];
			}
			else
				await api.post('/crm/activities', {
					item_id: id,
					kind: mode,
					subject: text,
					deadline: mode === 'meeting' ? undefined : when ? new Date(when).toISOString() : undefined,
					starts_at: mode === 'meeting' && when ? new Date(when).toISOString() : undefined,
					direction: mode === 'call' ? direction : undefined,
					responsible_id: resp ?? undefined
				});
			text = '';
			when = '';
			resp = null;
			respLabel = null;
			load();
		} catch (e) {
			toasts.error(e);
		} finally {
			posting = false;
		}
	}
	async function complete(a: Activity) {
		await api.post(`/crm/activities/${a.id}/complete`, {});
		load();
	}
	async function pin(e: Entry) {
		await api.patch(`/crm/timeline/${e.id}`, { is_pinned: !e.is_pinned });
		load();
	}
	async function older() {
		const last = timeline.items[timeline.items.length - 1];
		const t = await api.get<typeof timeline>(`/crm/items/${id}/timeline?before=${encodeURIComponent(last!.created_at)}`);
		timeline.items = [...timeline.items, ...t.items];
		timeline.has_more = t.has_more;
	}

	// ----- lead conversion -----
	let convertOpen = $state(false);
	let conv = $state({ create_contact: true, create_company: false, create_deal: true });
	async function convert() {
		try {
			const r = await api.post<{ deal_id: string | null; contact_id: string | null }>(`/crm/lead/items/${id}/convert`, conv);
			toasts.success('Lead converted');
			convertOpen = false;
			goto(r.deal_id ? `/crm/deal/${r.deal_id}` : `/crm/contact/${r.contact_id}`);
		} catch (e) {
			toasts.error(e);
		}
	}

	const ACT_ICON: Record<string, string> = { todo: 'checkSquare', call: 'phone', meeting: 'calendar', email: 'mail' };
	/** Who a timeline entry is by: incoming emails show their sender. */
	function who(e: Entry): string | null {
		if (e.kind === 'webform') return 'Website visitor';
		if (e.kind === 'shop_order') return 'The customer';
		if (e.kind === 'chat' && !e.author_name) return 'The visitor';
		if (e.kind === 'signature' && !e.author_name) return e.data.name ?? 'E-signature';
		if (e.kind === 'recurring' && !e.author_name) return 'Repeat schedule';
		if (e.kind === 'booking') return e.data.name ?? 'The client';
		if (e.kind === 'campaign') return e.data.event === 'unsubscribed' ? 'The client' : 'Marketing';
		return e.kind === 'email' && e.data.direction === 'in' ? e.data.from : e.author_name;
	}
	function describe(e: Entry): string {
		switch (e.kind) {
			case 'created':
				return `created this ${type?.name.toLowerCase()}`;
			case 'stage':
				return `moved it from ${e.data.from?.name ?? '—'} to ${e.data.to?.name}`;
			case 'field':
				return e.data.field === 'assigned_to' ? `made ${e.data.to} responsible` : `changed ${e.data.field}`;
			case 'converted':
				return e.data.from_lead ? `created this from the lead "${e.data.lead_title}"` : 'converted this lead';
			case 'merged':
				return `merged "${e.data.title}" into this record`;
			case 'booking':
				return e.data.cancelled ? `cancelled the meeting they booked` : `booked a meeting through “${e.data.link}” for ${formatDateTime(e.data.starts_at)}`;
			case 'restored':
				return 'restored this from the recycle bin';
			case 'products':
				return `updated products (${e.data.count} rows, total ${money(e.data.total, e.data.currency)})`;
			case 'robot':
				return e.data.status === 'failed' ? `could not run “${e.data.name}”: ${e.data.result}` : `ran “${e.data.name}”: ${e.data.result}`;
			case 'trigger':
				return `moved it by the trigger “${e.data.name}”`;
			case 'automation':
				return `stopped automation: ${e.data.reason}`;
			case 'sla':
				return `flagged it: more than ${durationText(e.data.minutes)} in “${e.data.stage?.name}”`;
			case 'edit':
				return `changed ${(e.data.changes as { field: string }[]).map((c) => historyLabel(c.field)).join(', ')}`;
			case 'observers':
				return [e.data.added?.length ? `added ${e.data.added.join(', ')} as observer${e.data.added.length > 1 ? 's' : ''}` : '', e.data.removed?.length ? `removed ${e.data.removed.join(', ')}` : ''].filter(Boolean).join('; ');
			case 'task':
				return `${e.data.event === 'completed' ? 'completed' : e.data.event === 'linked' ? 'linked' : 'created'} the task #${e.data.number} “${e.data.title}”`;
			case 'campaign':
				return e.data.event === 'unsubscribed' ? `unsubscribed from marketing emails (“${e.data.name}”)` : `sent the campaign “${e.data.name}”`;
			case 'linked':
				return `created ${typeName(e.data.entity_type).toLowerCase()} #${e.data.number} “${e.data.title}”`;
			case 'requisite':
				return `${e.data.event} requisites “${e.data.name}”`;
			case 'document':
				return `created ${e.data.number} from “${e.data.template}”`;
			case 'document_deleted':
				return `deleted ${e.data.number}`;
			case 'accounting':
				return e.data.event === 'voided' ? 'voided it in the books' : `posted it to the books (${money(e.data.total, e.data.currency)})`;
			case 'webform':
				return `submitted the form “${e.data.form}”`;
			case 'recurring':
				return e.data.event === 'set'
					? `set it to repeat: ${e.data.summary}`
					: e.data.event === 'stopped'
						? 'stopped it repeating'
						: e.data.event === 'made'
							? e.data.manual
								? `made a copy now: #${e.data.number}`
								: `created #${e.data.number} from it on schedule`
							: 'created this as a repeat';
			case 'calendar':
				return `planned “${e.data.title}” on ${e.data.all_day ? new Date(e.data.start).toLocaleDateString(undefined, { timeZone: 'UTC' }) : formatDateTime(e.data.start)}`;
			case 'signature':
				switch (e.data.event) {
					case 'sent':
						return `sent ${e.data.number} for signature to ${(e.data.signers as string[]).join(', ')}`;
					case 'signed':
						return `signed ${e.data.number}`;
					case 'declined':
						return `declined to sign ${e.data.number}${e.data.reason ? `: “${e.data.reason}”` : ''}`;
					case 'completed':
						return `${e.data.number} is signed by everyone`;
					default:
						return `cancelled the signature request for ${e.data.number}`;
				}
			case 'chat':
				return `chatted on the website (“${e.data.channel}”, ${e.data.count} message${e.data.count === 1 ? '' : 's'})`;
			case 'shop_order':
				return `placed this order in “${e.data.shop}” (${money(e.data.total, e.data.currency)})`;
			case 'email':
				return e.data.direction === 'in' ? 'wrote an email' : 'sent an email';
			case 'activity':
				return `${e.data.event === 'completed' ? 'completed' : 'planned'} a ${e.data.kind}: ${e.body}${e.data.result ? ` — ${e.data.result}` : ''}`;
			default:
				return e.kind;
		}
	}
</script>

<svelte:head><title>{isNew ? `New ${type?.name.toLowerCase() ?? ''}` : (item?.title ?? 'CRM')} · NERP</title></svelte:head>

<a href="/crm/{code}" class="mb-3 inline-flex items-center gap-1 text-sm text-slate-500 hover:text-slate-800">
	<Icon name="chevronRight" class="size-4 rotate-180" /> {type?.name_plural ?? 'CRM'}
</a>

{#if type && isNew}
	<div class="card max-w-4xl p-6">
		<h1 class="mb-5 text-2xl text-slate-900">New {type.name.toLowerCase()}</h1>
		{#if cardInfo}
			<CrmRecordForm {type} {defaults} card={cardInfo} onsaved={(d) => goto(`/crm/${code}/${d.id}`, { replace: true })} oncancel={() => history.back()} />
		{:else}
			<p class="text-sm text-slate-400">Loading…</p>
		{/if}
	</div>
{:else if notFound}
	<div class="card p-10 text-center text-slate-500">This record doesn't exist, was deleted, or you don't have access to it.</div>
{:else if type && item}
	<!-- Header -->
	<div class="card mb-5 p-5">
		<div class="flex flex-wrap items-start gap-4">
			<div class="min-w-0 flex-1">
				<p class="text-xs text-slate-400">{type.name} #{item.number}{category && type.categories.length > 1 ? ` · ${category.name}` : ''}</p>
				<div class="flex flex-wrap items-center gap-3">
					<h1 class="text-2xl text-slate-900">{item.title}</h1>
					<ChatCallButtons small chat={recordChat} call={false} />
					{#if !session.user?.is_guest}<BookingLink itemId={item.id} />{/if}
				</div>
				<p class="mt-1 flex flex-wrap gap-x-4 text-sm text-slate-500">
					{#if type.has_amount}<span class="font-medium text-slate-800">{money(item.opportunity, item.currency)}</span>{/if}
					{#if item.company_title && item.company_id}<a class="hover:underline" href="/crm/company/{item.company_id}">{item.company_title}</a>{/if}
					{#if item.job_title}<span>{item.job_title}</span>{/if}
					{#if item.assigned_name}<span class="flex items-center gap-1.5"><Avatar name={item.assigned_name} fileId={item.assigned_avatar_file_id} size="xs" />{item.assigned_name}</span>{/if}
				</p>
				<div class="mt-2 flex flex-wrap items-center gap-1.5 text-sm" aria-label="Observers">
					<span class="text-slate-500">Observers:</span>
					{#each item.observers as o (o.id)}
						<span class="inline-flex items-center gap-1 rounded-full bg-slate-100 py-0.5 pr-1 pl-1 text-xs text-slate-700">
							<Avatar name={o.full_name} fileId={o.avatar_file_id} size="xs" />{o.full_name}
							{#if item.can_write}<button class="rounded-full p-0.5 text-slate-400 hover:text-red-600" aria-label="Remove observer {o.full_name}" onclick={() => setObservers(item!.observers.filter((x) => x.id !== o.id).map((x) => x.id))}><Icon name="x" class="size-3" /></button>{/if}
						</span>
					{:else}
						<span class="text-xs text-slate-400">none</span>
					{/each}
					{#if item.can_write}
						<div class="w-48"><UserPicker placeholder="+ Add observer" exclude={item.observers.map((o) => o.id)} onpick={(u) => setObservers([...item!.observers.map((o) => o.id), u.id])} /></div>
					{/if}
				</div>
			</div>
			<div class="flex flex-wrap gap-2">
				{#each makeTargets as t (t.id)}
					<button class="btn btn-secondary" disabled={making} onclick={() => makeFrom(t)}><Icon name="plus" class="size-4" /> Create {t.name.toLowerCase()}</button>
				{/each}
				{#if code === 'lead' && item.stage_semantics === 'process' && item.can_write}
					<button class="btn btn-primary" onclick={() => (convertOpen = true)}>Convert</button>
				{/if}
				{#if item.can_write}<button class="btn btn-secondary" onclick={() => (editing = !editing)}><Icon name="edit" class="size-4" /> {editing ? 'Close editor' : 'Edit'}</button>{/if}
				{#if item.can_delete}<button class="btn btn-ghost p-2 text-slate-400 hover:text-red-600" aria-label="Delete" onclick={remove}><Icon name="trash" class="size-4" /></button>{/if}
			</div>
		</div>

		{#if type.has_stages && category}
			<!-- Stage bar: click to move; final stages as buttons -->
			<div class="mt-4 flex flex-wrap items-center gap-2">
				<div class="flex min-w-0 flex-1 overflow-hidden rounded-full">
					{#each openStages as s, i (s.id)}
						<button
							class="min-w-0 flex-1 truncate px-2 py-1.5 text-xs font-medium transition {item.stage_semantics === 'process' && i <= currentIdx ? 'text-white' : 'bg-slate-100 text-slate-500 hover:bg-slate-200'}"
							style={item.stage_semantics === 'process' && i <= currentIdx ? `background:${s.color}` : item.stage_semantics === 'success' ? `background:${s.color};color:white;opacity:.55` : ''}
							disabled={!item.can_write}
							title="Move to {s.name}"
							onclick={() => setStage(s.id)}>{s.name}</button
						>
					{/each}
				</div>
				{#each category.stages.filter((s) => s.semantics !== 'process') as s (s.id)}
					<button
						class="btn {item.stage_id === s.id ? 'text-white' : 'btn-secondary'}"
						style={item.stage_id === s.id ? `background:${s.color}` : ''}
						disabled={!item.can_write || item.stage_id === s.id}
						onclick={() => setStage(s.id)}>{s.name}</button
					>
				{/each}
			</div>
			{#if late !== null && currentStage?.sla_minutes}
				<p class="mt-2 flex items-center gap-1.5 text-sm text-red-700">
					<Icon name="clock" class="size-4" /> Over the time limit for “{currentStage.name}” ({durationText(currentStage.sla_minutes)}) by {durationText(late)}
				</p>
			{/if}
		{/if}
	</div>

	{#if editing}
		<div class="card mb-5 max-w-4xl p-6">
			{#if cardInfo}
				<CrmRecordForm {type} {item} card={cardInfo} onsaved={(d) => ((item = d), (editing = false), load())} oncancel={() => (editing = false)} />
			{:else}
				<p class="text-sm text-slate-400">Loading…</p>
			{/if}
		</div>
	{/if}

	{#if type.has_amount && item.currency}
		<div class="mb-5"><ProductRows itemId={item.id} currency={item.currency} canEdit={item.can_write} onchanged={load} /></div>
	{/if}

	<div class="grid gap-5 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)]">
		<!-- Left: data -->
		<div class="space-y-5">
			{#if item.children.length}
				<section class="card" aria-labelledby="made-h">
					<div class="flex flex-wrap items-baseline gap-x-4 border-b border-slate-100 px-5 py-3">
						<h2 id="made-h" class="font-medium text-slate-900">{item.children.every((c) => c.entity_type === quoteType?.id || c.entity_type === invoiceType?.id) ? 'Quotes and invoices' : 'Made from this record'}</h2>
						{#if billing}
							<p class="text-sm text-slate-500">Invoiced <span class="font-medium text-slate-800">{money(String(billing.invoiced), item.currency)}</span> · paid <span class="font-medium text-slate-800">{money(String(billing.paid), item.currency)}</span></p>
						{/if}
					</div>
					<ul class="divide-y divide-slate-50">
						{#each item.children as c (c.id)}
							{@const ct = crm.config?.types.find((t) => t.id === c.entity_type)}
							<li>
								<a class="flex items-center gap-3 px-5 py-2.5 text-sm hover:bg-slate-50" href={itemLink(c.entity_type, c.id)}>
									<span class="w-24 shrink-0 text-slate-500">{typeName(c.entity_type)} #{c.number}</span>
									<span class="min-w-0 flex-1 truncate text-slate-800">{c.title}</span>
									{#if invoiceOverdue(ct, c)}<span class="rounded bg-red-50 px-1.5 py-0.5 text-xs font-medium text-red-700">Overdue</span>{/if}
									{#if c.stage_name}<span class="rounded-full px-2 py-0.5 text-xs font-medium text-white" style="background:{c.stage_color}">{c.stage_name}</span>{/if}
									{#if c.currency}<span class="w-28 shrink-0 text-right font-medium text-slate-800 tabular-nums">{money(c.opportunity, c.currency)}</span>{/if}
								</a>
							</li>
						{/each}
					</ul>
				</section>
			{/if}
			{#if type.kind === 'invoice' && (session.can(PERM.ACCOUNTING_VIEW) || session.can(PERM.ACCOUNTING_POST))}
				<InvoiceBooksCard itemId={item.id} onchanged={load} />
			{/if}
			{#if type.has_stages && !['lead', 'contact', 'company'].includes(type.code)}
				<RecurringCard itemId={item.id} canEdit={item.can_write} dateLabel={type.kind === 'quote' ? 'Valid until' : type.kind === 'order' ? 'Deliver by' : type.code === 'deal' ? 'Close date' : 'Due date'} onchanged={load} />
			{/if}
			{#if shopOrder}
				<section class="card" aria-labelledby="shop-h" data-shop-order>
					<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3">
						<h2 id="shop-h" class="font-medium text-slate-900">From the online shop</h2>
						<a class="text-sm font-medium text-brand-600 hover:underline" href={shopOrder.order_url} target="_blank" rel="noopener">Customer's order page</a>
					</div>
					<dl class="grid gap-x-4 gap-y-2 px-5 py-4 text-sm sm:grid-cols-[8rem_1fr]">
						<dt class="text-slate-500">Customer</dt><dd class="text-slate-800">{shopOrder.name}</dd>
						<dt class="text-slate-500">Email</dt><dd><a class="text-brand-600 hover:underline" href="mailto:{shopOrder.email}">{shopOrder.email}</a></dd>
						{#if shopOrder.phone}<dt class="text-slate-500">Phone</dt><dd class="text-slate-800">{shopOrder.phone}</dd>{/if}
						<dt class="text-slate-500">Deliver to</dt><dd class="whitespace-pre-line text-slate-800">{shopOrder.address}</dd>
						{#if shopOrder.note}<dt class="text-slate-500">Note</dt><dd class="whitespace-pre-line text-slate-800">{shopOrder.note}</dd>{/if}
					</dl>
					<p class="border-t border-slate-100 px-5 py-2.5 text-xs text-slate-500">
						{shopOrder.stock_returned ? 'Cancelled: its units are back in stock.' : 'Its units were taken off the stock at checkout. Cancelling the order puts them back.'} The customer gets an email when the order changes stage.
					</p>
				</section>
			{/if}
			{#if cardInfo}
				<CardSections {type} {item} info={cardInfo} onchange={(i) => (cardInfo = i)} />
			{/if}

			{#if type.code !== 'contact' || item.contacts.length}
				<section class="card">
					<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3">
						<h2 class="font-medium text-slate-900">Contacts</h2>
						{#if type.code === 'company'}<a class="text-sm font-medium text-brand-600 hover:underline" href="/crm/contact/new?company_id={item.id}&company_title={encodeURIComponent(item.title)}">+ New contact</a>{/if}
					</div>
					<ul class="divide-y divide-slate-50">
						{#each item.contacts as c (c.id)}
							<li class="px-5 py-2.5 text-sm"><a class="text-slate-800 hover:text-brand-700" href="/crm/contact/{c.id}">{c.title}</a>{#if c.is_primary}<span class="badge ml-2 bg-slate-100 text-slate-500">primary</span>{/if}</li>
						{:else}
							<li class="px-5 py-3 text-sm text-slate-400">No contacts linked.</li>
						{/each}
					</ul>
				</section>
			{/if}

			{#if item.related.length || type.code === 'contact' || type.code === 'company'}
				<section class="card">
					<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3">
						<h2 class="font-medium text-slate-900">Deals{type.code === 'company' ? ', leads & contacts' : ' & leads'}</h2>
						<a class="text-sm font-medium text-brand-600 hover:underline" href="/crm/deal/new?{type.code === 'company' ? `company_id=${item.id}&company_title=${encodeURIComponent(item.title)}` : `contact_id=${item.id}&contact_title=${encodeURIComponent(item.title)}`}">+ New deal</a>
					</div>
					<ul class="divide-y divide-slate-50">
						{#each item.related as r (r.id)}
							<li class="flex items-center gap-3 px-5 py-2.5 text-sm">
								<span class="badge bg-slate-100 text-slate-500">{typeName(r.entity_type)}</span>
								<a class="min-w-0 flex-1 truncate text-slate-800 hover:text-brand-700" href={itemLink(r.entity_type, r.id)}>{r.title}</a>
								{#if r.stage_name}<span class="badge text-white" style="background:{r.stage_color}">{r.stage_name}</span>{/if}
								{#if r.entity_type !== 3}<span class="text-slate-600">{money(r.opportunity, r.currency)}</span>{/if}
							</li>
						{:else}
							<li class="px-5 py-3 text-sm text-slate-400">Nothing yet.</li>
						{/each}
					</ul>
				</section>
			{/if}

			{#if type.code === 'company' || type.code === 'contact'}
				<RequisitesCard itemId={item.id} person={type.code === 'contact'} canWrite={item.can_write} defaultCountry={item.address?.country ?? ''} defaultName={item.title} />
			{/if}
			{#if type.code === 'contact' || type.code === 'company'}{#key item.id}<ClientEverything itemId={item.id} />{/key}{/if}
			{#if type.code === 'deal'}{#key item.id}<DealHealth itemId={item.id} /><PromiseCheck itemId={item.id} closeDate={item.close_date ?? null} assigned={item.assigned_to ? { id: item.assigned_to, name: item.assigned_name ?? '', avatar_file_id: item.assigned_avatar_file_id ?? null } : null} onDate={load} />{/key}{/if}
			<DocumentsCard itemId={item.id} typeCode={type.code} canWrite={item.can_write} hasBuyer={type.has_client} />
			{#if ['contact', 'company', 'deal'].includes(type.code)}
				{@const mf = (kind: string) => item!.multifields.find((m) => m.kind === kind)?.value ?? null}
				{#key item.id}<BookingsCard
					itemId={item.id}
					typeCode={type.code}
					clientName={type.code === 'deal' ? (item.contacts[0]?.title ?? item.company_title ?? item.title) : item.title}
					clientEmail={type.code === 'contact' ? mf('email') : null}
					clientPhone={type.code === 'contact' ? mf('phone') : null}
					contactId={type.code === 'contact' ? item.id : (type.code === 'deal' ? (item.contacts[0]?.id ?? null) : null)}
				/>{/key}
			{/if}
			{#key item.id}<RecordProcesses itemId={item.id} canWrite={item.can_write} />{/key}
			{#key item.id}<Journey itemId={item.id} />{/key}

			{#if item.source_item}
				<p class="text-sm text-slate-500">Created from <a class="text-brand-600 hover:underline" href={itemLink(item.source_item.entity_type, item.source_item.id)}>{item.source_item.title}</a></p>
			{/if}
		</div>

		<!-- Right: activity -->
		<div class="space-y-5">
			<section class="card p-4">
				<div class="mb-3 flex gap-1">
					{#each [['comment', 'Comment', 'message'], ['email', 'Email', 'mail'], ['todo', 'To-do', 'checkSquare'], ['call', 'Call', 'phone'], ['meeting', 'Meeting', 'calendar']] as [k, label, icon] (k)}
						<button class="btn px-3 py-1.5 {mode === k ? 'bg-brand-100 text-brand-900' : 'btn-ghost'}" onclick={() => ((mode = k as typeof mode), k === 'email' && (replyTo = null), composerKey++)}><Icon name={icon as 'message'} class="size-4" /> {label}</button>
					{/each}
				</div>
				{#if mode === 'email'}
					{#key composerKey}
						<EmailComposer itemId={item.id} entityType={type.id} reply={replyTo} onsent={() => ((replyTo = null), (mode = 'comment'), load())} oncancel={() => ((replyTo = null), (mode = 'comment'))} />
					{/key}
				{:else}
				<textarea class="input" rows="2" placeholder={mode === 'comment' ? 'Leave a comment…' : mode === 'call' ? 'What is the call about?' : mode === 'meeting' ? 'Meeting subject' : 'What needs to be done?'} bind:value={text}></textarea>
				{#if mode !== 'comment'}
					<div class="mt-2 flex flex-wrap items-center gap-2">
						<input type="datetime-local" class="input w-56" bind:value={when} aria-label={mode === 'meeting' ? 'Starts' : 'Deadline'} />
						{#if mode === 'call'}
							<select class="input w-36" bind:value={direction} aria-label="Direction"><option value="outgoing">Outgoing</option><option value="incoming">Incoming</option></select>
						{/if}
						<div class="w-56"><UserPicker bind:value={resp} bind:label={respLabel} placeholder="Responsible: me" /></div>
					</div>
					{#if mode === 'call' && !when}<p class="mt-1 text-xs text-slate-500">Without a time, the call is logged as already done.</p>{/if}
				{/if}
				{#if mode === 'comment' && attached.length}
					<ul class="mt-2 flex flex-wrap gap-2 text-xs" data-attached>
						{#each attached as f, i (f.id)}
							<li class="flex items-center gap-1 rounded bg-slate-100 px-2 py-1">{f.name}<button class="text-slate-500 hover:text-red-600" aria-label="Remove {f.name}" onclick={() => attached.splice(i, 1)}>×</button></li>
						{/each}
					</ul>
				{/if}
				<div class="mt-2 flex items-center justify-end gap-2">
					{#if mode === 'comment'}
						<label class="btn btn-ghost mr-auto cursor-pointer" title="Attach files">
							<Icon name="upload" class="size-4" /> {uploading ? 'Uploading…' : 'Attach'}
							<input type="file" class="sr-only" multiple onchange={attach} disabled={uploading} data-comment-files />
						</label>
					{/if}
					<button class="btn btn-primary" disabled={posting || uploading || (!text.trim() && !(mode === 'comment' && attached.length))} onclick={post}>{mode === 'comment' ? 'Post' : 'Save'}</button>
				</div>
				{/if}
			</section>

			{#if waiting.length}
				<section class="card">
					<h2 class="border-b border-slate-100 px-5 py-3 font-medium text-slate-900">Automation waiting</h2>
					<ul class="divide-y divide-slate-50">
						{#each waiting as w (w.id)}
							<li class="flex items-center gap-3 px-5 py-2.5 text-sm">
								<Icon name="clock" class="size-4 text-slate-400" /><span class="flex-1 text-slate-800">{w.robot_name}{#if w.is_test}<span class="badge ml-1.5 bg-amber-100 text-amber-800">test</span>{/if}</span>
								<span class="text-xs text-slate-500">
									{#if w.status === 'pending'}after the previous robot{:else if w.status === 'waiting'}waiting{w.wait_until ? ` until ${formatDateTime(w.wait_until)} at most` : ''}{:else}{formatDateTime(w.run_at)}{/if}
								</span>
							</li>
						{/each}
					</ul>
				</section>
			{/if}

			<section class="card" aria-labelledby="tasks-h">
				<div class="flex items-center border-b border-slate-100 px-5 py-3">
					<h2 id="tasks-h" class="flex-1 font-medium text-slate-900">Tasks</h2>
					<button class="btn btn-ghost px-2 py-1 text-sm" onclick={openTask}><Icon name="plus" class="size-4" /> Task</button>
				</div>
				{#if tasks.length}
					<ul class="divide-y divide-slate-50">
						{#each tasks as t (t.id)}
							<li>
								<a class="flex items-center gap-3 px-5 py-2.5 text-sm hover:bg-slate-50" href="/tasks/{t.id}">
									<Icon name={t.status === 'done' ? 'check' : 'checkSquare'} class="size-4 {t.status === 'done' ? 'text-emerald-600' : 'text-slate-400'}" />
									<span class="min-w-0 flex-1 truncate {t.status === 'done' ? 'text-slate-400 line-through' : 'text-slate-800'}">#{t.number} {t.title}</span>
									{#if t.deadline && t.status !== 'done'}<span class="text-xs {t.is_overdue ? 'font-medium text-red-600' : 'text-slate-500'}">{formatDateTime(t.deadline)}</span>{/if}
									<Avatar name={t.responsible_name} fileId={t.responsible_avatar_file_id} size="xs" />
								</a>
							</li>
						{/each}
					</ul>
				{:else}
					<p class="px-5 py-3 text-sm text-slate-500">No tasks yet.</p>
				{/if}
			</section>

			{#if activities.length}
				<section class="card">
					<h2 class="border-b border-slate-100 px-5 py-3 font-medium text-slate-900">Planned</h2>
					<ul class="divide-y divide-slate-50">
						{#each activities as a (a.id)}
							<li class="flex items-start gap-3 px-5 py-3 text-sm">
								<input type="checkbox" class="mt-0.5 rounded border-slate-300 text-emerald-600" aria-label="Mark done" onchange={() => complete(a)} />
								<Icon name={ACT_ICON[a.kind] as 'phone'} class="mt-0.5 size-4 text-slate-400" />
								<div class="min-w-0 flex-1">
									<p class="text-slate-800">{a.subject}</p>
									<p class="text-xs {a.is_overdue ? 'font-medium text-red-600' : 'text-slate-500'}">
										{a.deadline ? formatDateTime(a.deadline) : 'No deadline'}{a.responsible_id !== session.user?.id ? ` · ${a.responsible_name}` : ''}
									</p>
								</div>
							</li>
						{/each}
					</ul>
				</section>
			{/if}

			<section class="card">
				<h2 class="border-b border-slate-100 px-5 py-3 font-medium text-slate-900">Timeline</h2>
				<ul class="divide-y divide-slate-50">
					{#each [...timeline.pinned, ...timeline.items.filter((e) => !e.is_pinned)] as e (e.id)}
						<li class="group flex gap-3 px-5 py-3 text-sm {e.is_pinned ? 'bg-amber-50/50' : ''}">
							<Avatar name={who(e) ?? 'NERP'} fileId={e.author_avatar_file_id} size="sm" />
							<div class="min-w-0 flex-1">
								<p>
									<span class="font-medium text-slate-900">{who(e) ?? 'System'}</span>
									{#if e.kind !== 'comment'}<span class="text-slate-600"> {describe(e)}</span>{/if}
									<span class="ml-1 text-xs text-slate-400" title={formatDateTime(e.created_at)}>{formatRelative(e.created_at)}</span>
								</p>
								{#if e.kind === 'comment' && e.body}<p class="mt-1 whitespace-pre-line text-slate-700">{e.body}</p>{/if}
								{#if e.files?.length}
									<ul class="mt-1 flex flex-wrap gap-2" data-entry-files>
										{#each e.files as f (f.id)}
											<li><button class="flex items-center gap-1 rounded border border-slate-200 px-2 py-1 text-xs text-brand-700 hover:bg-slate-50" onclick={() => downloadFile(f.id, f.name).catch(toasts.error)}><Icon name="file" class="size-3.5" />{f.name} <span class="text-slate-400">{formatBytes(f.size)}</span></button></li>
										{/each}
									</ul>
								{/if}
								{#if e.kind === 'edit'}
									<ul class="mt-1 space-y-0.5 text-xs text-slate-600" data-history>
										{#each e.data.changes as c (c.field)}
											<li><span class="text-slate-500">{historyLabel(c.field)}:</span> <span class="line-through decoration-slate-300">{historyValue(c.field, c.from)}</span> → <span class="text-slate-800">{historyValue(c.field, c.to)}</span></li>
										{/each}
									</ul>
								{/if}
								{#if e.kind === 'task'}<a class="text-xs font-medium text-brand-700 hover:underline" href="/tasks/{e.data.task_id}">Open task #{e.data.number}</a>{/if}
								{#if e.kind === 'linked'}<a class="text-xs font-medium text-brand-700 hover:underline" href="/crm/{e.data.code}/{e.data.item_id}">Open #{e.data.number}</a>{/if}
								{#if e.kind === 'webform'}
									<dl class="mt-1 grid grid-cols-[minmax(0,9rem)_1fr] gap-x-3 text-xs">
										{#each e.data.values as v (v.key)}<dt class="truncate text-slate-500">{v.label}</dt><dd class="whitespace-pre-line text-slate-800">{v.value}</dd>{/each}
									</dl>
								{/if}
								{#if e.kind === 'chat'}
									<details class="mt-1 text-xs" data-transcript>
										<summary class="cursor-pointer text-slate-500">Transcript</summary>
										<div class="mt-1 space-y-1">
											{#each e.data.messages as m, i (i)}<p><span class="font-medium {m.visitor ? 'text-slate-800' : 'text-brand-700'}">{m.who}:</span> <span class="whitespace-pre-line text-slate-700">{m.body}</span></p>{/each}
										</div>
									</details>
								{/if}
								{#if e.kind === 'email'}
									<button class="mt-1 block w-full text-left" onclick={() => (openEmails[e.id] = !openEmails[e.id])}>
										<span class="font-medium text-slate-800">{e.body || '(no subject)'}</span>
										{#if !openEmails[e.id] && e.data.snippet}<span class="block truncate text-xs text-slate-500">{e.data.snippet}</span>{/if}
									</button>
									{#if openEmails[e.id]}<EmailView emailId={e.data.email_id} onreply={startReply} />{/if}
								{/if}
							</div>
							<button class="self-start text-slate-300 {e.is_pinned ? 'text-amber-500' : 'invisible group-hover:visible'} hover:text-amber-600" title={e.is_pinned ? 'Unpin' : 'Pin'} aria-label="Pin" onclick={() => pin(e)}>📌</button>
						</li>
					{/each}
				</ul>
				{#if timeline.has_more}<button class="w-full border-t border-slate-100 py-2.5 text-sm font-medium text-brand-600 hover:bg-slate-50" onclick={older}>Show older</button>{/if}
			</section>
		</div>
	</div>

	<Modal bind:open={convertOpen} title="Convert lead">
		<div class="space-y-3 text-sm">
			<p class="text-slate-600">Create records from <strong>{item.title}</strong>. Its details, contact information and products carry over.</p>
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={conv.create_contact} /> Contact {item.first_name || item.last_name ? `(${[item.first_name, item.last_name].filter(Boolean).join(' ')})` : ''}</label>
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" disabled={!item.company_name} bind:checked={conv.create_company} /> Company {item.company_name ? `(${item.company_name})` : '— the lead has no company name'}</label>
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={conv.create_deal} /> Deal {Number(item.opportunity) ? `(${money(item.opportunity, item.currency)})` : ''}</label>
		</div>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (convertOpen = false)}>Cancel</button>
			<button class="btn btn-primary" disabled={!conv.create_contact && !conv.create_company && !conv.create_deal} onclick={convert}>Convert</button>
		{/snippet}
	</Modal>
{:else}
	<div class="card p-10 text-center text-slate-400">Loading…</div>
{/if}

<Modal bind:open={taskOpen} title="New task" size="md">
	<form id="task-form" class="space-y-3 text-sm" onsubmit={addTask}>
		<div><label class="label" for="nt-title">Task</label><input id="nt-title" class="input" required maxlength="500" bind:value={newTask.title} /></div>
		<div class="grid gap-3 sm:grid-cols-2">
			<div><label class="label" for="nt-due">Deadline</label><input id="nt-due" class="input" type="datetime-local" bind:value={newTask.deadline} /></div>
			<div><span class="label">Responsible</span><UserPicker bind:value={newTask.responsible_id} bind:label={newTask.responsible_name} /></div>
		</div>
		<p class="text-xs text-slate-500">The task is linked to this {type?.name.toLowerCase()}; it shows here and in Tasks.</p>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (taskOpen = false)}>Cancel</button>
		<button class="btn btn-primary" type="submit" form="task-form">Create task</button>
	{/snippet}
</Modal>
