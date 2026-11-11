<script lang="ts">
	// CRM settings → Automation, per pipeline:
	// * Rules: one column per stage with its time limit, the triggers that move records into
	//   it and the robots that run when a record arrives (or relative to a date).
	// * Schedules: actions run daily, weekly or monthly for matching records.
	// * Log and Time in stages: what automation did, and how long records stay.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import { crm, durationText, type CrmCategory, type CrmType } from '#lib/crm.svelte.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { User } from '#lib/types.ts';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';
	import UserPicker from '../UserPicker.svelte';
	import MultiUserPicker, { type PickedUser } from '../MultiUserPicker.svelte';
	import AutomationLog from './AutomationLog.svelte';
	import StageTimes from './StageTimes.svelte';

	type Condition = { field: string; op: string; value: string };
	type Wait = { until: 'conditions' | 'email_received' | 'activity_done'; conditions: Condition[]; timeout_minutes: number; on_timeout: 'run' | 'skip' };
	type Robot = {
		id: string; stage_id: string; name: string; action: string; params: Record<string, any>; delay_minutes: number; conditions: Condition[]; is_active: boolean; sort: number;
		timing: 'stage' | 'field'; timing_field: string | null; offset_minutes: number; after_previous: boolean; wait: Wait | null;
	};
	type Trigger = { id: string; stage_id: string; name: string; event: string; params: Record<string, any>; conditions: Condition[]; allow_backwards: boolean; token: string | null; is_active: boolean };
	type Schedule = {
		id: string; name: string; frequency: 'daily' | 'weekly' | 'monthly'; weekdays: number[]; month_day: number; at_time: string; timezone: string; stage_ids: string[];
		conditions: Condition[]; action: string; params: Record<string, any>; is_active: boolean; next_run_at: string | null; last_run_at: string | null; last_count: number | null;
	};

	const ACTIONS: [string, string, string][] = [
		['activity', 'Plan an activity', 'checkSquare'],
		['task', 'Create a task', 'list'],
		['notify', 'Notify employees', 'bell'],
		['email', 'Email the client', 'mail'],
		['set_field', 'Change a field', 'edit'],
		['assign', 'Change the responsible person', 'user'],
		['move_stage', 'Move to another stage', 'chevronRight'],
		['tunnel', 'Tunnel to another pipeline', 'chevronRight'],
		['document', 'Create a document', 'file'],
		['sign', 'Send for signature', 'edit'],
		['comment', 'Add a comment', 'message'],
		['webhook', 'Call a webhook', 'send'],
		['create_record', 'Create a linked record', 'plus'],
		['convert_lead', 'Convert the lead', 'refresh'],
		['observers', 'Add observers', 'eye']
	];
	const EVENTS: [string, string][] = [
		['activity_done', 'An activity is completed'],
		['comment_added', 'A comment is added'],
		['document_created', 'A document is created'],
		['email_received', 'An email arrives from the client'],
		['webform_submitted', 'A web form is submitted'],
		['webhook', 'An external system calls a webhook'],
		['signature_completed', 'Everyone signs a document'],
		['signature_declined', 'Someone declines to sign'],
		['tasks_done', 'All its tasks are done']
	];
	const WAITS: [Wait['until'], string][] = [
		['conditions', 'the conditions below hold'],
		['email_received', 'the client replies by email'],
		['activity_done', 'an activity is completed']
	];
	const OPS: [string, string][] = [['eq', '='], ['ne', '≠'], ['gt', '>'], ['gte', '≥'], ['lt', '<'], ['lte', '≤'], ['contains', 'contains'], ['empty', 'is empty'], ['not_empty', 'is not empty']];
	const PLACEHOLDERS = ['title', 'number', 'amount', 'stage', 'assigned', 'company', 'contact', 'client_email', 'link'];
	const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
	const UNITS: [number, string][] = [[1, 'minutes'], [60, 'hours'], [1440, 'days']];
	const timezones: string[] = Intl.supportedValuesOf?.('timeZone') ?? ['UTC'];

	let view = $state<'rules' | 'schedules' | 'log' | 'times'>('rules');
	const types = $derived((crm.config?.types ?? []).filter((t) => t.has_stages) as CrmType[]);
	let typeCode = $state('deal');
	const type = $derived(types.find((t) => t.code === typeCode));
	let categoryId = $state('');
	const category = $derived(type?.categories.find((c) => c.id === categoryId) as CrmCategory | undefined);
	$effect(() => {
		// Default pipeline whenever the type changes.
		const t = type;
		untrack(() => {
			if (t && !t.categories.some((c) => c.id === categoryId)) categoryId = (t.categories.find((c) => c.is_default) ?? t.categories[0])?.id ?? '';
		});
	});

	let robots = $state<Robot[]>([]);
	let triggers = $state<Trigger[]>([]);
	let schedules = $state<Schedule[]>([]);
	let dateFields = $state<{ code: string; label: string }[]>([]);
	let webhookBase = $state('');
	// Test mode: the pipeline's automation runs only for the testers' records.
	let testMode = $state({ enabled: false, users: [] as PickedUser[] });
	let testSaved = $state({ enabled: false, ids: '' });
	const testDirty = $derived(testMode.enabled !== testSaved.enabled || testMode.users.map((u) => u.id).sort().join() !== testSaved.ids);
	async function saveTestMode() {
		try {
			const r = await api.put<{ enabled: boolean; user_ids: string[] }>('/crm/automation/test-mode', { category_id: categoryId, enabled: testMode.enabled, user_ids: testMode.users.map((u) => u.id) });
			testSaved = { enabled: r.enabled, ids: [...r.user_ids].sort().join() };
			toasts.success(r.enabled ? 'Test mode on: automation runs only for the testers’ records' : 'Test mode off: automation runs for every record');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function load() {
		if (!categoryId) return;
		const [r, s] = await Promise.all([
			api.get<{ robots: Robot[]; triggers: Trigger[]; webhook_base: string; date_fields: typeof dateFields; test_mode: { enabled: boolean; user_ids: string[] } }>(`/crm/automation?category_id=${categoryId}`),
			api.get<{ items: Schedule[] }>(`/crm/schedules?category_id=${categoryId}`)
		]);
		robots = r.robots;
		triggers = r.triggers;
		webhookBase = r.webhook_base;
		dateFields = r.date_fields;
		schedules = s.items;
		const testers = await Promise.all(
			r.test_mode.user_ids.map((id) => api.get<{ id: string; first_name: string; last_name: string; avatar_file_id: string | null }>(`/users/${id}`).catch(() => null))
		);
		testMode = { enabled: r.test_mode.enabled, users: testers.filter((u) => !!u).map((u) => ({ id: u.id, name: `${u.first_name} ${u.last_name}`, avatar_file_id: u.avatar_file_id })) };
		testSaved = { enabled: r.test_mode.enabled, ids: [...r.test_mode.user_ids].sort().join() };
	}
	$effect(() => {
		categoryId;
		untrack(load);
	});

	// Names for ids shown in the editor (employees, templates, projects).
	let names = $state<Record<string, string>>({});
	async function userName(id: string) {
		if (names[id] || id === 'assigned' || id === 'creator') return;
		try {
			const u = await api.get<User>(`/users/${id}`);
			names[id] = u.full_name;
		} catch {
			names[id] = 'Unknown employee';
		}
	}
	let templates = $state<{ id: string; name: string; entity_types: number[] }[]>([]);
	let projects = $state<{ id: string; name: string }[]>([]);
	let signTemplates = $state<{ id: string; name: string; entity_types: number[] }[]>([]);
	let extrasLoaded = false;
	async function loadExtras() {
		if (extrasLoaded) return;
		extrasLoaded = true;
		[templates, projects, signTemplates] = await Promise.all([
			api.get<{ items: typeof templates }>('/crm/doc-templates').then((r) => r.items),
			api.get<typeof projects>('/projects?mine=true').catch(() => []),
			api.get<{ items: typeof signTemplates }>('/crm/sign-templates').then((r) => r.items).catch(() => [])
		]);
	}
	const typeSignTemplates = $derived(signTemplates.filter((t) => type && (!t.entity_types.length || t.entity_types.includes(type.id))));
	const typeTemplates = $derived(templates.filter((t) => type && t.entity_types.includes(type.id)));
	// Record types a robot can create: leads, deals and smart processes.
	const creatable = $derived((crm.config?.types ?? []).filter((t) => t.code === 'deal' || t.code === 'lead' || !t.is_system) as CrmType[]);

	const fields = $derived([
		...(type?.system_fields.filter((f) => !['multifield', 'address'].includes(f.kind)).map((f) => ({ code: f.code, label: f.label, kind: f.kind })) ?? []),
		...(type?.custom_fields.map((f) => ({ code: `uf.${f.code}`, label: f.label, kind: f.kind })) ?? [])
	]);
	const settable = $derived(fields.filter((f) => f.code !== 'assigned_to'));
	const stageName = (id: string) => category?.stages.find((s) => s.id === id)?.name ?? '?';

	/** 0 → "", 90 → "1 h 30 min". */
	const amount = (m: number) => (m ? durationText(Math.abs(m)) : '');
	function bestUnit(m: number): number {
		if (m && m % 1440 === 0) return 1440;
		if (m && m % 60 === 0) return 60;
		return 1;
	}
	function whenText(r: Robot): string {
		if (r.timing === 'field') {
			const f = dateFields.find((d) => d.code === r.timing_field)?.label ?? r.timing_field;
			return r.offset_minutes ? `${amount(r.offset_minutes)} ${r.offset_minutes < 0 ? 'before' : 'after'} ${f}` : `at ${f}`;
		}
		return r.delay_minutes ? `after ${amount(r.delay_minutes)}` : '';
	}
	function waitText(w: Wait): string {
		const what = w.until === 'conditions' ? `${w.conditions.length} condition${w.conditions.length === 1 ? '' : 's'}` : w.until === 'email_received' ? 'a client reply' : 'an activity';
		return `waits for ${what}, ${amount(w.timeout_minutes)} max`;
	}

	// ----- editor -----
	type ActionDraft = { action: string; params: Record<string, any>; conditions: Condition[] };
	type RobotDraft = ActionDraft & {
		kind: 'robot'; id: string | null; stage_id: string; name: string; is_active: boolean;
		when: 'now' | 'delay' | 'field'; delay: number; unit: number; field: string; offset: number; offsetUnit: number; direction: 'before' | 'after';
		after_previous: boolean; waits: boolean; wait: Wait; waitAmount: number; waitUnit: number;
	};
	type TriggerDraft = { kind: 'trigger'; id: string | null; stage_id: string; name: string; event: string; params: Record<string, any>; conditions: Condition[]; allow_backwards: boolean; is_active: boolean; token: string | null };
	type ScheduleDraft = ActionDraft & {
		kind: 'schedule'; id: string | null; name: string; is_active: boolean; frequency: Schedule['frequency']; weekdays: number[]; month_day: number; at_time: string; timezone: string; stage_ids: string[];
	};
	type Draft = RobotDraft | TriggerDraft | ScheduleDraft;
	let draft = $state<Draft | null>(null);
	let open = $state(false);
	let saving = $state(false);

	function defaults(action: string): Record<string, any> {
		switch (action) {
			case 'activity': return { kind: 'call', subject: 'Call about {{title}}', description: '', deadline_minutes: 60, responsible: 'assigned' };
			case 'task': return { title: 'Follow up: {{title}}', description: '', deadline_minutes: 1440, responsible: 'assigned', project_id: '' };
			case 'notify': return { to: ['assigned'], message: '{{title}} moved to {{stage}}' };
			case 'email': return { subject: 'About {{title}}', body: 'Hello,\n\n…\n\nBest regards,\n{{assigned}}', template_id: '' };
			case 'set_field': return { field: settable[0]?.code ?? 'probability', value: '' };
			case 'assign': return { users: [], mode: 'round_robin' };
			case 'move_stage': return { stage_id: category?.stages.find((s) => s.semantics === 'process')?.id ?? '' };
			case 'document': return { template_id: typeTemplates[0]?.id ?? '' };
			case 'sign': return { sign_template_id: typeSignTemplates[0]?.id ?? '' };
			case 'tunnel': {
				const other = type?.categories.find((c) => c.id !== categoryId);
				return { stage_id: other?.stages.find((s) => s.semantics === 'process')?.id ?? '', mode: 'copy' };
			}
			case 'webhook': return { url: 'https://', method: 'POST', headers: [], body: '', secret: '' };
			case 'create_record': return { entity_type: creatable.find((t) => t.code === 'deal')?.id ?? creatable[0]?.id, category_id: '', title: 'Follow-up: {{title}}', responsible: 'assigned', copy_client: true, copy_amount: false };
			case 'convert_lead': return { create_contact: true, create_company: false, create_deal: true, deal_category_id: '' };
			case 'observers': return { users: [] };
			default: return { text: '' };
		}
	}
	const actionsFor = $derived(ACTIONS.filter(([k]) => k !== 'convert_lead' || typeCode === 'lead'));
	const emptyWait = (): Wait => ({ until: 'conditions', conditions: [], timeout_minutes: 1440, on_timeout: 'run' });

	async function newRobot(stage: string) {
		await loadExtras();
		draft = {
			kind: 'robot', id: null, stage_id: stage, name: '', action: 'activity', params: defaults('activity'), conditions: [], is_active: true,
			when: 'now', delay: 1, unit: 60, field: dateFields[0]?.code ?? 'created_at', offset: 1, offsetUnit: 1440, direction: 'before',
			after_previous: false, waits: false, wait: emptyWait(), waitAmount: 1, waitUnit: 1440
		};
		open = true;
	}
	async function editRobot(r: Robot) {
		await loadExtras();
		const unit = bestUnit(r.delay_minutes);
		const offsetUnit = bestUnit(Math.abs(r.offset_minutes)) || 1440;
		const wait: Wait = r.wait ? { ...r.wait, conditions: r.wait.conditions.map((c) => ({ ...c, value: String(c.value ?? '') })) } : emptyWait();
		const waitUnit = bestUnit(wait.timeout_minutes);
		draft = {
			kind: 'robot', id: r.id, stage_id: r.stage_id, name: r.name, action: r.action, params: structuredClone($state.snapshot(r.params)),
			conditions: r.conditions.map((c) => ({ ...c, value: String(c.value ?? '') })), is_active: r.is_active,
			when: r.timing === 'field' ? 'field' : r.delay_minutes ? 'delay' : 'now', delay: r.delay_minutes / unit || 1, unit,
			field: r.timing_field ?? dateFields[0]?.code ?? 'created_at', offset: Math.abs(r.offset_minutes) / offsetUnit, offsetUnit, direction: r.offset_minutes > 0 ? 'after' : 'before',
			after_previous: r.after_previous, waits: !!r.wait, wait, waitAmount: wait.timeout_minutes / waitUnit, waitUnit
		};
		for (const id of [...(r.params.to ?? []), ...(r.params.users ?? []), r.params.responsible].filter(Boolean)) userName(id);
		open = true;
	}
	async function newTrigger(stage: string) {
		await loadExtras();
		draft = { kind: 'trigger', id: null, stage_id: stage, name: '', event: 'activity_done', params: {}, conditions: [], allow_backwards: false, is_active: true, token: null };
		open = true;
	}
	async function editTrigger(t: Trigger) {
		await loadExtras();
		draft = { kind: 'trigger', id: t.id, stage_id: t.stage_id, name: t.name, event: t.event, params: { ...t.params }, conditions: t.conditions.map((c) => ({ ...c, value: String(c.value ?? '') })), allow_backwards: t.allow_backwards, is_active: t.is_active, token: t.token };
		open = true;
	}
	const myTz = () => (session.user?.timezone && session.user.timezone !== 'UTC' ? session.user.timezone : Intl.DateTimeFormat().resolvedOptions().timeZone);
	async function newSchedule() {
		await loadExtras();
		draft = {
			kind: 'schedule', id: null, name: '', action: 'move_stage', params: defaults('move_stage'), conditions: [], is_active: true,
			frequency: 'weekly', weekdays: [1], month_day: 1, at_time: '09:00', timezone: myTz(), stage_ids: []
		};
		open = true;
	}
	async function editSchedule(s: Schedule) {
		await loadExtras();
		draft = {
			kind: 'schedule', id: s.id, name: s.name, action: s.action, params: structuredClone($state.snapshot(s.params)), conditions: s.conditions.map((c) => ({ ...c, value: String(c.value ?? '') })),
			is_active: s.is_active, frequency: s.frequency, weekdays: [...s.weekdays], month_day: s.month_day, at_time: s.at_time.slice(0, 5), timezone: s.timezone, stage_ids: [...s.stage_ids]
		};
		for (const id of [...(s.params.to ?? []), ...(s.params.users ?? []), s.params.responsible].filter(Boolean)) userName(id);
		open = true;
	}

	function robotBody(d: RobotDraft) {
		const label = ACTIONS.find((a) => a[0] === d.action)?.[1];
		const offset = Math.round(Number(d.offset || 0) * d.offsetUnit) * (d.direction === 'before' ? -1 : 1);
		return {
			category_id: categoryId, stage_id: d.stage_id, name: d.name.trim() || label, action: d.action, params: d.params, conditions: d.conditions, is_active: d.is_active,
			timing: d.when === 'field' ? 'field' : 'stage', timing_field: d.when === 'field' ? d.field : null, offset_minutes: d.when === 'field' ? offset : 0,
			delay_minutes: d.when === 'delay' ? Math.round(Number(d.delay || 0) * d.unit) : 0,
			after_previous: d.after_previous,
			wait: d.waits ? { ...d.wait, conditions: d.wait.until === 'conditions' ? d.wait.conditions : [], timeout_minutes: Math.round(Number(d.waitAmount || 0) * d.waitUnit) } : null
		};
	}

	async function save() {
		const d = draft;
		if (!d) return;
		saving = true;
		try {
			if (d.kind === 'robot') {
				const body = robotBody(d);
				if (d.id) await api.patch(`/crm/robots/${d.id}`, body);
				else await api.post('/crm/robots', body);
			} else if (d.kind === 'trigger') {
				const label = EVENTS.find((e) => e[0] === d.event)?.[1];
				const body = { category_id: categoryId, stage_id: d.stage_id, name: d.name.trim() || label, event: d.event, params: d.params, conditions: d.conditions, allow_backwards: d.allow_backwards, is_active: d.is_active };
				if (d.id) await api.patch(`/crm/triggers/${d.id}`, body);
				else await api.post('/crm/triggers', body);
			} else {
				const label = ACTIONS.find((a) => a[0] === d.action)?.[1];
				const body = {
					category_id: categoryId, name: d.name.trim() || label, action: d.action, params: d.params, conditions: d.conditions, is_active: d.is_active,
					frequency: d.frequency, weekdays: d.weekdays, month_day: Number(d.month_day), at_time: d.at_time, timezone: d.timezone, stage_ids: d.stage_ids
				};
				if (d.id) await api.patch(`/crm/schedules/${d.id}`, body);
				else await api.post('/crm/schedules', body);
			}
			open = false;
			await load();
			toasts.success('Automation saved');
		} catch (e) {
			toasts.error(e);
		} finally {
			saving = false;
		}
	}

	const PATHS = { robot: 'robots', trigger: 'triggers', schedule: 'schedules' } as const;
	async function remove() {
		if (!draft?.id || !confirm(`Delete “${draft.name}”?`)) return;
		try {
			await api.del(`/crm/${PATHS[draft.kind]}/${draft.id}`);
			open = false;
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	async function toggle(kind: 'robots' | 'triggers' | 'schedules', id: string, is_active: boolean) {
		try {
			await api.patch(`/crm/${kind}/${id}`, { is_active });
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	/** Moves a robot up or down within its stage (the order robots run in). */
	async function move(r: Robot, by: -1 | 1) {
		const list = robots.filter((x) => x.stage_id === r.stage_id);
		const i = list.findIndex((x) => x.id === r.id);
		const j = i + by;
		if (j < 0 || j >= list.length) return;
		[list[i], list[j]] = [list[j], list[i]];
		try {
			await api.post('/crm/robot-order', { stage_id: r.stage_id, ids: list.map((x) => x.id) });
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	async function runNow(s: Schedule) {
		if (!confirm(`Run “${s.name}” now for every matching record?`)) return;
		try {
			const r = await api.post<{ count: number }>(`/crm/schedules/${s.id}/run`, {});
			toasts.success(r.count ? `Started for ${r.count} record${r.count === 1 ? '' : 's'}` : 'No records match right now');
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- stage time limits -----
	let sla = $state<{ stage_id: string; name: string; on: boolean; amount: number; unit: number; escalate: boolean } | null>(null);
	let slaOpen = $state(false);
	function editSla(s: CrmCategory['stages'][number]) {
		const m = s.sla_minutes ?? 0;
		const unit = m ? bestUnit(m) : 1440;
		sla = { stage_id: s.id, name: s.name, on: !!m, amount: m ? m / unit : 2, unit, escalate: s.sla_escalate };
		slaOpen = true;
	}
	async function saveSla() {
		if (!sla) return;
		try {
			await api.patch(`/crm/stages/${sla.stage_id}`, { sla_minutes: sla.on ? Math.round(Number(sla.amount) * sla.unit) : null, sla_escalate: sla.escalate });
			slaOpen = false;
			await crm.load(true);
			toasts.success('Time limit saved');
		} catch (e) {
			toasts.error(e);
		}
	}

	// ----- fields a stage asks for before records may enter it -----
	const REQUIRABLE = ['opportunity', 'company_id', 'contact_ids', 'close_date', 'probability', 'source', 'source_details', 'comments', 'first_name', 'last_name', 'company_name', 'job_title', 'assigned_to'];
	const requirable = $derived(fields.filter((f) => REQUIRABLE.includes(f.code) || f.code.startsWith('uf.')));
	let req = $state<{ stage_id: string; name: string; fields: string[] } | null>(null);
	let reqOpen = $state(false);
	function editRequired(s: CrmCategory['stages'][number]) {
		req = { stage_id: s.id, name: s.name, fields: [...(s.required_fields ?? [])] };
		reqOpen = true;
	}
	async function saveRequired() {
		if (!req) return;
		try {
			await api.patch(`/crm/stages/${req.stage_id}`, { required_fields: req.fields });
			reqOpen = false;
			await crm.load(true);
			toasts.success('Required fields saved');
		} catch (e) {
			toasts.error(e);
		}
	}

	function personLabel(v: string) {
		return v === 'assigned' ? 'Responsible person' : v === 'creator' ? 'Creator' : (names[v] ?? '…');
	}
	function summary(r: { action: string; params: Record<string, any> }): string {
		const p = r.params;
		switch (r.action) {
			case 'activity': return `${p.kind}: ${p.subject}`;
			case 'task': return p.title;
			case 'notify': return p.message;
			case 'email': return p.template_id ? `${p.subject} + ${templates.find((t) => t.id === p.template_id)?.name ?? 'document'}` : p.subject;
			case 'set_field': return `${fields.find((f) => f.code === p.field)?.label ?? p.field} → ${p.value}`;
			case 'assign': return `${p.users.length} employee${p.users.length === 1 ? '' : 's'}, ${p.mode === 'random' ? 'random' : p.mode === 'least_busy' ? 'least busy' : 'in turn'}`;
			case 'move_stage': return `→ ${stageName(p.stage_id)}`;
			case 'document': return templates.find((t) => t.id === p.template_id)?.name ?? 'Template';
			case 'sign': return signTemplates.find((t) => t.id === p.sign_template_id)?.name ?? 'Signing template';
			case 'tunnel': {
				const c = type?.categories.find((x) => x.stages.some((s) => s.id === p.stage_id));
				return `${p.mode === 'move' ? 'Move' : 'Copy'} to ${c?.name ?? '?'}: ${c?.stages.find((s) => s.id === p.stage_id)?.name ?? '?'}`;
			}
			case 'webhook': return `${p.method} ${p.url}`;
			case 'create_record': return `${(crm.config?.types ?? []).find((t) => t.id === p.entity_type)?.name ?? 'Record'}: ${p.title}`;
			case 'convert_lead': return [p.create_contact && 'contact', p.create_company && 'company', p.create_deal && 'deal'].filter(Boolean).join(' + ');
			case 'observers': return (p.users as string[]).map(personLabel).join(', ');
			default: return p.text ?? '';
		}
	}
	function scheduleText(s: { frequency: string; weekdays: number[]; month_day: number; at_time: string; timezone: string }): string {
		const at = s.at_time.slice(0, 5);
		if (s.frequency === 'daily') return `Every day at ${at}`;
		if (s.frequency === 'weekly') return `Every ${s.weekdays.map((d) => DAYS[d - 1]).join(', ')} at ${at}`;
		return `Monthly on day ${s.month_day} at ${at}`;
	}
	function insertPlaceholder(key: string, ph: string) {
		if (draft) draft.params[key] = `${draft.params[key] ?? ''}{{${ph}}}`;
	}
	const ad = () => draft as ActionDraft;
</script>

{#snippet placeholders(key: string)}
	<div class="mt-1 flex flex-wrap gap-1">
		{#each [...PLACEHOLDERS, ...(type?.custom_fields.map((f) => `uf.${f.code}`) ?? [])] as ph (ph)}
			<button type="button" class="rounded bg-slate-100 px-1.5 py-0.5 font-mono text-[11px] text-slate-600 hover:bg-brand-50 hover:text-brand-700" onclick={() => insertPlaceholder(key, ph)}>{`{{${ph}}}`}</button>
		{/each}
	</div>
{/snippet}

{#snippet personSelect(key: string, id: string)}
	<select {id} class="input" value={['assigned', 'creator'].includes(draft!.params[key]) ? draft!.params[key] : 'user'} onchange={(e) => (draft!.params[key] = e.currentTarget.value === 'user' ? '' : e.currentTarget.value)}>
		<option value="assigned">The record's responsible person</option>
		<option value="creator">The record's creator</option>
		<option value="user">A specific employee…</option>
	</select>
	{#if !['assigned', 'creator'].includes(draft!.params[key])}
		<div class="mt-2">
			{#if draft!.params[key]}<p class="mb-1 text-xs text-slate-600">{personLabel(draft!.params[key])}</p>{/if}
			<UserPicker placeholder="Choose employee…" onpick={(u) => ((draft!.params[key] = u.id), (names[u.id] = u.full_name))} />
		</div>
	{/if}
{/snippet}

{#snippet conditionList(list: Condition[], title: string, hint: string)}
	<div class="space-y-2">
		<div class="flex items-center justify-between">
			<span class="label mb-0">{title} <span class="font-normal text-slate-400">({hint})</span></span>
			<button type="button" class="btn btn-ghost px-2 py-1 text-xs" onclick={() => list.push({ field: fields[0]?.code ?? 'title', op: 'eq', value: '' })}><Icon name="plus" class="size-3.5" /> Add condition</button>
		</div>
		{#each list as c, i (i)}
			<div class="flex gap-2">
				<select class="input flex-1" bind:value={c.field} aria-label="Field">
					{#each fields as f (f.code)}<option value={f.code}>{f.label}</option>{/each}
				</select>
				<select class="input w-32" bind:value={c.op} aria-label="Operator">
					{#each OPS as [k, l] (k)}<option value={k}>{l}</option>{/each}
				</select>
				{#if c.op !== 'empty' && c.op !== 'not_empty'}<input class="input w-36" bind:value={c.value} aria-label="Value" />{/if}
				<button type="button" class="text-slate-300 hover:text-red-600" aria-label="Remove condition" onclick={() => list.splice(i, 1)}><Icon name="x" class="size-4" /></button>
			</div>
		{/each}
		{#if list.some((c) => ['close_date', 'created_at'].includes(c.field) || fields.find((f) => f.code === c.field)?.kind.startsWith('date'))}
			<p class="text-xs text-slate-500">For dates, compare with <code>today</code>, <code>today+7</code>, <code>today-3</code> or <code>now</code>.</p>
		{/if}
	</div>
{/snippet}

{#snippet amountUnit(label: string, get: () => number, set: (v: number) => void, getUnit: () => number, setUnit: (v: number) => void)}
	<input class="input w-20" type="number" min="1" value={get()} oninput={(e) => set(Number(e.currentTarget.value))} aria-label={label} />
	<select class="input w-28" value={getUnit()} onchange={(e) => setUnit(Number(e.currentTarget.value))} aria-label="{label} unit">
		{#each UNITS as [v, l] (v)}<option value={v}>{l}</option>{/each}
	</select>
{/snippet}

{#snippet actionEditor()}
	{@const p = ad().params}
	{@const action = ad().action}
	<div class="space-y-3 rounded-xl bg-slate-50 p-3">
		{#if action === 'activity'}
			<div class="grid gap-3 sm:grid-cols-2">
				<div>
					<label class="label" for="au-kind">Activity</label>
					<select id="au-kind" class="input" bind:value={p.kind}><option value="todo">To-do</option><option value="call">Call</option><option value="meeting">Meeting</option></select>
				</div>
				<div>
					<label class="label" for="au-due">Due in (minutes)</label>
					<input id="au-due" class="input" type="number" min="0" bind:value={p.deadline_minutes} />
				</div>
			</div>
			<div><label class="label" for="au-subject">Subject</label><input id="au-subject" class="input" bind:value={p.subject} />{@render placeholders('subject')}</div>
			<div><label class="label" for="au-who">Responsible</label>{@render personSelect('responsible', 'au-who')}</div>
		{:else if action === 'task'}
			<div><label class="label" for="au-title">Task title</label><input id="au-title" class="input" bind:value={p.title} />{@render placeholders('title')}</div>
			<div><label class="label" for="au-desc">Description</label><textarea id="au-desc" class="input" rows="3" bind:value={p.description}></textarea></div>
			<div class="grid gap-3 sm:grid-cols-2">
				<div><label class="label" for="au-due">Deadline in (minutes, 0 = none)</label><input id="au-due" class="input" type="number" min="0" bind:value={p.deadline_minutes} /></div>
				<div>
					<label class="label" for="au-proj">Project</label>
					<select id="au-proj" class="input" bind:value={p.project_id}><option value="">None</option>{#each projects as pr (pr.id)}<option value={pr.id}>{pr.name}</option>{/each}</select>
				</div>
			</div>
			<div><label class="label" for="au-who">Responsible</label>{@render personSelect('responsible', 'au-who')}</div>
		{:else if action === 'notify'}
			<div>
				<span class="label">Who</span>
				<div class="flex flex-wrap gap-1.5">
					{#each p.to as v, i (v + i)}
						<span class="badge flex items-center gap-1 bg-white text-slate-700">{personLabel(v)}<button type="button" aria-label="Remove" onclick={() => p.to.splice(i, 1)}><Icon name="x" class="size-3" /></button></span>
					{/each}
				</div>
				<div class="mt-2 flex flex-wrap gap-2">
					{#if !p.to.includes('assigned')}<button type="button" class="btn btn-ghost px-2 py-1 text-xs" onclick={() => p.to.push('assigned')}>+ Responsible person</button>{/if}
					{#if !p.to.includes('creator')}<button type="button" class="btn btn-ghost px-2 py-1 text-xs" onclick={() => p.to.push('creator')}>+ Creator</button>{/if}
					<div class="w-56"><UserPicker placeholder="+ Employee…" exclude={p.to} onpick={(u) => (p.to.push(u.id), (names[u.id] = u.full_name))} /></div>
				</div>
			</div>
			<div><label class="label" for="au-msg">Message</label><input id="au-msg" class="input" bind:value={p.message} />{@render placeholders('message')}</div>
		{:else if action === 'email'}
			<p class="text-xs text-slate-500">Sent to the record's email, else its primary contact's, else its company's. Logged as an email activity.</p>
			<div><label class="label" for="au-subj">Subject</label><input id="au-subj" class="input" bind:value={p.subject} />{@render placeholders('subject')}</div>
			<div><label class="label" for="au-body">Message</label><textarea id="au-body" class="input" rows="6" bind:value={p.body}></textarea>{@render placeholders('body')}</div>
			<div>
				<label class="label" for="au-att">Attach a document</label>
				<select id="au-att" class="input" value={p.template_id ?? ''} onchange={(e) => (p.template_id = e.currentTarget.value || null)}>
					<option value="">No attachment</option>
					{#each typeTemplates as t (t.id)}<option value={t.id}>{t.name}</option>{/each}
				</select>
				{#if p.template_id}<p class="mt-1 text-xs text-slate-500">The document is created first; the email goes out once its PDF is ready.</p>{/if}
			</div>
		{:else if action === 'set_field'}
			<div class="grid gap-3 sm:grid-cols-2">
				<div><label class="label" for="au-field">Field</label><select id="au-field" class="input" bind:value={p.field}>{#each settable as f (f.code)}<option value={f.code}>{f.label}</option>{/each}</select></div>
				<div><label class="label" for="au-val">New value</label><input id="au-val" class="input" bind:value={p.value} /></div>
			</div>
		{:else if action === 'assign'}
			<div>
				<span class="label">Employees</span>
				<div class="flex flex-wrap gap-1.5">
					{#each p.users as v, i (v)}
						<span class="badge flex items-center gap-1 bg-white text-slate-700">{personLabel(v)}<button type="button" aria-label="Remove" onclick={() => p.users.splice(i, 1)}><Icon name="x" class="size-3" /></button></span>
					{/each}
				</div>
				<div class="mt-2 w-64"><UserPicker placeholder="+ Employee…" exclude={p.users} onpick={(u) => (p.users.push(u.id), (names[u.id] = u.full_name))} /></div>
			</div>
			<div class="flex flex-wrap gap-4">
				<label class="flex items-center gap-1.5"><input type="radio" class="text-brand-600" value="round_robin" bind:group={p.mode} /> In turn</label>
				<label class="flex items-center gap-1.5"><input type="radio" class="text-brand-600" value="least_busy" bind:group={p.mode} /> Fewest open records</label>
				<label class="flex items-center gap-1.5"><input type="radio" class="text-brand-600" value="random" bind:group={p.mode} /> At random</label>
			</div>
		{:else if action === 'move_stage'}
			<div><label class="label" for="au-to">Move to</label><select id="au-to" class="input" bind:value={p.stage_id}>{#each category?.stages ?? [] as s (s.id)}<option value={s.id}>{s.name}</option>{/each}</select></div>
		{:else if action === 'document'}
			<div>
				<label class="label" for="au-tpl">Template</label>
				<select id="au-tpl" class="input" bind:value={p.template_id}>{#each typeTemplates as t (t.id)}<option value={t.id}>{t.name}</option>{/each}</select>
			</div>
		{:else if action === 'tunnel'}
			<div class="grid gap-3 sm:grid-cols-[1fr_9rem]">
				<div>
					<label class="label" for="au-tun">To</label>
					<select id="au-tun" class="input" bind:value={p.stage_id}>
						{#each (type?.categories ?? []).filter((c) => c.id !== categoryId) as c (c.id)}
							<optgroup label={c.name}>{#each c.stages as s (s.id)}<option value={s.id}>{s.name}</option>{/each}</optgroup>
						{/each}
					</select>
				</div>
				<div><label class="label" for="au-tunm">How</label><select id="au-tunm" class="input" bind:value={p.mode}><option value="copy">Copy it there</option><option value="move">Move it there</option></select></div>
			</div>
			<p class="text-xs text-slate-500">A copy is linked to this record and keeps its client, amount, products and custom fields.</p>
		{:else if action === 'sign'}
			<div>
				<label class="label" for="au-stpl">Signing template</label>
				<select id="au-stpl" class="input" bind:value={p.sign_template_id}>{#each typeSignTemplates as t (t.id)}<option value={t.id}>{t.name}</option>{/each}</select>
				<p class="mt-1 text-xs text-slate-500">Sent in the name of the record's responsible person. Set up signing templates under Documents.</p>
			</div>
		{:else if action === 'webhook'}
			<div class="grid gap-3 sm:grid-cols-[8rem_1fr]">
				<div><label class="label" for="au-method">Method</label><select id="au-method" class="input" bind:value={p.method}>{#each ['POST', 'PUT', 'PATCH', 'GET', 'DELETE'] as m (m)}<option>{m}</option>{/each}</select></div>
				<div><label class="label" for="au-url">URL</label><input id="au-url" class="input font-mono text-xs" bind:value={p.url} placeholder="https://example.com/hooks/crm" />{@render placeholders('url')}</div>
			</div>
			<div>
				<div class="flex items-center justify-between">
					<span class="label mb-0">Headers</span>
					<button type="button" class="btn btn-ghost px-2 py-1 text-xs" onclick={() => p.headers.push({ name: '', value: '' })}><Icon name="plus" class="size-3.5" /> Add header</button>
				</div>
				{#each p.headers as h, i (i)}
					<div class="mt-2 flex gap-2">
						<input class="input w-44 font-mono text-xs" bind:value={h.name} placeholder="Authorization" aria-label="Header name" />
						<input class="input flex-1 font-mono text-xs" bind:value={h.value} placeholder="Bearer …" aria-label="Header value" />
						<button type="button" class="text-slate-300 hover:text-red-600" aria-label="Remove header" onclick={() => p.headers.splice(i, 1)}><Icon name="x" class="size-4" /></button>
					</div>
				{/each}
			</div>
			{#if p.method !== 'GET' && p.method !== 'DELETE'}
				<div>
					<label class="label" for="au-wbody">Body</label>
					<textarea id="au-wbody" class="input font-mono text-xs" rows="5" bind:value={p.body} placeholder={'Empty: the whole record as JSON.\n{"deal": "{{title}}", "amount": "{{opportunity}}"}'}></textarea>
					{@render placeholders('body')}
					<p class="mt-1 text-xs text-slate-500">A body starting with {'{'} or [ is sent as JSON, with values escaped; anything else as plain text.</p>
				</div>
				<div>
					<label class="label" for="au-secret">Signing secret <span class="font-normal text-slate-400">(optional)</span></label>
					<input id="au-secret" class="input font-mono text-xs" bind:value={p.secret} autocomplete="off" />
					<p class="mt-1 text-xs text-slate-500">Adds <code>X-NERP-Signature: sha256=…</code>, an HMAC of the body, so the receiver can check the call came from here.</p>
				</div>
			{/if}
			<p class="text-xs text-slate-500">Server errors and timeouts are retried after 1, 5 and 30 minutes. Redirects are not followed.</p>
		{:else if action === 'create_record'}
			<div class="grid gap-3 sm:grid-cols-2">
				<div>
					<label class="label" for="au-ctype">Create a</label>
					<select id="au-ctype" class="input" value={p.entity_type} onchange={(e) => ((p.entity_type = Number(e.currentTarget.value)), (p.category_id = ''))}>
						{#each creatable as t (t.id)}<option value={t.id}>{t.name}</option>{/each}
					</select>
				</div>
				{#if (creatable.find((t) => t.id === p.entity_type)?.categories.length ?? 0) > 1}
					<div>
						<label class="label" for="au-ccat">Pipeline</label>
						<select id="au-ccat" class="input" bind:value={p.category_id}>
							<option value="">Default</option>
							{#each creatable.find((t) => t.id === p.entity_type)?.categories ?? [] as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
						</select>
					</div>
				{/if}
			</div>
			<div><label class="label" for="au-ctitle">Title</label><input id="au-ctitle" class="input" bind:value={p.title} />{@render placeholders('title')}</div>
			<div><label class="label" for="au-cwho">Responsible</label>{@render personSelect('responsible', 'au-cwho')}</div>
			<div class="flex flex-wrap gap-4">
				<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={p.copy_client} /> Same company and contacts</label>
				<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={p.copy_amount} /> Same amount</label>
			</div>
			<p class="text-xs text-slate-500">The new record shows where it came from, and this record's timeline links to it.</p>
		{:else if action === 'observers'}
			<div>
				<span class="label">Who follows the record</span>
				<div class="flex flex-wrap gap-1.5">
					{#each p.users as v, i (v + i)}
						<span class="badge flex items-center gap-1 bg-white text-slate-700">{personLabel(v)}<button type="button" aria-label="Remove" onclick={() => p.users.splice(i, 1)}><Icon name="x" class="size-3" /></button></span>
					{/each}
				</div>
				<div class="mt-2 flex flex-wrap gap-2">
					{#if !p.users.includes('creator')}<button type="button" class="btn btn-ghost px-2 py-1 text-xs" onclick={() => p.users.push('creator')}>+ Creator</button>{/if}
					<div class="w-56"><UserPicker placeholder="+ Employee…" exclude={p.users} onpick={(u) => (p.users.push(u.id), (names[u.id] = u.full_name))} /></div>
				</div>
				<p class="mt-1 text-xs text-slate-500">Observers can open the record and are told about stage changes and comments.</p>
			</div>
		{:else if action === 'convert_lead'}
			<div class="flex flex-wrap gap-4">
				<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={p.create_contact} /> Contact</label>
				<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={p.create_company} /> Company</label>
				<label class="flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={p.create_deal} /> Deal</label>
			</div>
			{#if p.create_deal && (crm.type('deal')?.categories.length ?? 0) > 1}
				<div>
					<label class="label" for="au-dcat">Deal pipeline</label>
					<select id="au-dcat" class="input" bind:value={p.deal_category_id}>
						<option value="">Default</option>
						{#each crm.type('deal')?.categories ?? [] as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
					</select>
				</div>
			{/if}
			<p class="text-xs text-slate-500">The lead moves to its successful stage. A company needs the lead's company name.</p>
		{:else}
			<div><label class="label" for="au-text">Comment</label><textarea id="au-text" class="input" rows="3" bind:value={p.text}></textarea>{@render placeholders('text')}</div>
		{/if}
	</div>
{/snippet}

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-2">
		<select class="input w-40" bind:value={typeCode} aria-label="Record type">
			{#each types as t (t.code)}<option value={t.code}>{t.name_plural}</option>{/each}
		</select>
		{#if type && type.categories.length > 1}
			<select class="input w-56" bind:value={categoryId} aria-label="Pipeline">
				{#each type.categories as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
			</select>
		{/if}
		<div class="ml-auto flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist" aria-label="Automation view">
			{#each [['rules', 'Rules'], ['schedules', 'Schedules'], ['log', 'Log'], ['times', 'Time in stages']] as [k, l] (k)}
				<button role="tab" aria-selected={view === k} class="rounded-full px-3 py-1 {view === k ? 'bg-white font-medium text-slate-900 shadow-g1' : 'text-slate-600 hover:text-slate-900'}" onclick={() => (view = k as typeof view)}>{l}</button>
			{/each}
		</div>
	</div>

	{#if view === 'rules'}
		<p class="text-sm text-slate-500">Robots run when a record enters their stage, or relative to one of its dates. Triggers move records forward when something happens.</p>
		<section class="flex flex-wrap items-center gap-3 rounded-xl px-4 py-3 text-sm {testSaved.enabled ? 'bg-amber-50 ring-1 ring-amber-200' : 'bg-slate-50'}" data-test-mode aria-label="Test mode">
			<label class="flex items-center gap-2 font-medium text-slate-800"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={testMode.enabled} /> Test mode</label>
			<span class="text-slate-500">{testMode.enabled ? 'Only records whose responsible person is a tester run this pipeline’s robots, triggers and schedules.' : 'Try new rules on a few people’s records before everyone’s.'}</span>
			{#if testMode.enabled}<div class="w-full max-w-md"><MultiUserPicker bind:value={testMode.users} placeholder="Testers" /></div>{/if}
			{#if testDirty}<button class="btn btn-primary ml-auto" onclick={saveTestMode} disabled={testMode.enabled && !testMode.users.length}>Save</button>{/if}
		</section>
		{#if category}
			<div class="flex gap-3 overflow-x-auto pb-3">
				{#each category.stages as s (s.id)}
					{@const stageRobots = robots.filter((r) => r.stage_id === s.id)}
					<section class="flex w-64 shrink-0 flex-col rounded-2xl bg-white shadow-g1" aria-label="Stage {s.name}">
						<h3 class="rounded-t-2xl px-4 py-2 text-sm font-medium text-white" style="background:{s.color}">{s.name}</h3>
						{#if s.semantics === 'process'}
							<button class="flex items-center gap-1.5 border-b border-slate-100 px-4 py-1.5 text-left text-xs {s.sla_minutes ? 'text-red-700' : 'text-slate-400 hover:text-slate-700'}" onclick={() => editSla(s)}>
								<Icon name="clock" class="size-3.5" />
								{s.sla_minutes ? `Time limit ${durationText(s.sla_minutes)}${s.sla_escalate ? ', escalates' : ''}` : '+ Time limit'}
							</button>
						{/if}
						<button class="flex items-center gap-1.5 border-b border-slate-100 px-4 py-1.5 text-left text-xs {s.required_fields?.length ? 'text-slate-700' : 'text-slate-400 hover:text-slate-700'}" onclick={() => editRequired(s)} data-required-fields>
							<Icon name="check" class="size-3.5" />
							{s.required_fields?.length ? `Requires ${s.required_fields.length} field${s.required_fields.length === 1 ? '' : 's'}` : '+ Required fields'}
						</button>
						<div class="space-y-1.5 border-b border-slate-100 p-2">
							{#each triggers.filter((t) => t.stage_id === s.id) as t (t.id)}
								<button class="flex w-full items-center gap-2 rounded-lg bg-amber-50 px-2.5 py-1.5 text-left text-xs text-amber-900 hover:bg-amber-100 {t.is_active ? '' : 'opacity-50'}" onclick={() => editTrigger(t)}>
									<Icon name="play" class="size-3.5 shrink-0" /><span class="truncate">{t.name}</span>
								</button>
							{/each}
							<button class="w-full rounded-lg px-2.5 py-1 text-left text-xs text-slate-400 hover:bg-slate-50 hover:text-slate-700" onclick={() => newTrigger(s.id)}>+ Trigger</button>
						</div>
						<div class="flex-1 space-y-2 p-2">
							{#each stageRobots as r, i (r.id)}
								{#if r.after_previous && i > 0}<p class="-my-1 pl-3 text-[11px] text-slate-400">↓ then</p>{/if}
								<div class="group rounded-xl border border-slate-200 p-2.5 text-xs {r.is_active ? '' : 'opacity-50'}">
									<button class="w-full text-left" onclick={() => editRobot(r)}>
										<p class="flex items-center gap-1.5 font-medium text-slate-800"><Icon name={(ACTIONS.find((a) => a[0] === r.action)?.[2] ?? 'cpu') as 'cpu'} class="size-3.5 shrink-0 text-brand-600" /><span class="truncate">{r.name}</span></p>
										<p class="mt-0.5 truncate text-slate-500">{summary(r)}</p>
										{#if whenText(r) || r.wait || r.conditions.length}
											<p class="mt-1 flex flex-wrap gap-x-2 text-[11px] text-slate-400">
												{#if whenText(r)}<span>⏱ {whenText(r)}</span>{/if}
												{#if r.wait}<span>⌛ {waitText(r.wait)}</span>{/if}
												{#if r.conditions.length}<span>if {r.conditions.length} condition{r.conditions.length > 1 ? 's' : ''}</span>{/if}
											</p>
										{/if}
									</button>
									<div class="mt-1.5 flex items-center gap-1 text-[11px] text-slate-500">
										<label class="flex items-center gap-1"><input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={r.is_active} onchange={(e) => toggle('robots', r.id, e.currentTarget.checked)} /> Active</label>
										{#if stageRobots.length > 1}
											<span class="ml-auto flex opacity-0 group-hover:opacity-100 focus-within:opacity-100">
												<button class="rounded px-1 hover:bg-slate-100 disabled:opacity-30" disabled={i === 0} aria-label="Move up" onclick={() => move(r, -1)}>↑</button>
												<button class="rounded px-1 hover:bg-slate-100 disabled:opacity-30" disabled={i === stageRobots.length - 1} aria-label="Move down" onclick={() => move(r, 1)}>↓</button>
											</span>
										{/if}
									</div>
								</div>
							{/each}
							<button class="w-full rounded-lg border border-dashed border-slate-300 px-2.5 py-2 text-xs text-slate-500 hover:border-brand-400 hover:text-brand-700" onclick={() => newRobot(s.id)}>+ Robot</button>
						</div>
					</section>
				{/each}
			</div>
		{/if}
	{:else if view === 'schedules'}
		<div class="flex items-center gap-3">
			<p class="flex-1 text-sm text-slate-500">Schedules run an action for every record of this pipeline that is in the chosen stages and matches the conditions — for example, each Monday move overdue records to a follow-up stage.</p>
			<button class="btn btn-primary" onclick={newSchedule}><Icon name="plus" class="size-4" /> Schedule</button>
		</div>
		{#if schedules.length}
			<div class="card divide-y divide-slate-100">
				{#each schedules as s (s.id)}
					<div class="flex flex-wrap items-center gap-3 px-5 py-3 text-sm {s.is_active ? '' : 'opacity-60'}">
						<button class="min-w-0 flex-1 text-left" onclick={() => editSchedule(s)}>
							<p class="flex items-center gap-1.5 font-medium text-slate-900"><Icon name={(ACTIONS.find((a) => a[0] === s.action)?.[2] ?? 'cpu') as 'cpu'} class="size-4 text-brand-600" />{s.name}</p>
							<p class="mt-0.5 text-xs text-slate-500">
								{scheduleText(s)} ({s.timezone}) · {s.stage_ids.length ? s.stage_ids.map(stageName).join(', ') : 'all open stages'}{s.conditions.length ? ` · ${s.conditions.length} condition${s.conditions.length > 1 ? 's' : ''}` : ''} · {summary(s)}
							</p>
						</button>
						<div class="text-right text-xs text-slate-500">
							{#if s.next_run_at}<p>Next: {formatDateTime(s.next_run_at)}</p>{:else}<p>Paused</p>{/if}
							{#if s.last_run_at}<p>Last: {formatDateTime(s.last_run_at)} · {s.last_count ?? 0} record{s.last_count === 1 ? '' : 's'}</p>{/if}
						</div>
						<label class="flex items-center gap-1 text-xs text-slate-500"><input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={s.is_active} onchange={(e) => toggle('schedules', s.id, e.currentTarget.checked)} /> Active</label>
						<button class="btn btn-secondary px-3 py-1 text-xs" onclick={() => runNow(s)}><Icon name="play" class="size-3.5" /> Run now</button>
					</div>
				{/each}
			</div>
		{:else}
			<div class="card p-8 text-center text-sm text-slate-500">No schedules for this pipeline yet.</div>
		{/if}
	{:else if view === 'log' && category}
		<AutomationLog categoryId={category.id} {typeCode} />
	{:else if view === 'times' && category}
		<StageTimes categoryId={category.id} />
	{/if}
</div>

<Modal bind:open title={draft?.kind === 'trigger' ? (draft.id ? 'Edit trigger' : 'New trigger') : draft?.kind === 'schedule' ? (draft.id ? 'Edit schedule' : 'New schedule') : draft?.id ? 'Edit robot' : 'New robot'} size="lg">
	{#if draft}
		<div class="space-y-4 text-sm">
			<div class="grid gap-3 sm:grid-cols-2">
				{#if draft.kind !== 'trigger'}
					<div>
						<label class="label" for="au-action">Action</label>
						<select id="au-action" class="input" value={draft.action} disabled={!!draft.id} onchange={(e) => { const d = draft as ActionDraft; d.action = e.currentTarget.value; d.params = defaults(d.action); }}>
							{#each actionsFor as [k, l] (k)}<option value={k}>{l}</option>{/each}
						</select>
					</div>
				{:else}
					<div>
						<label class="label" for="au-event">When</label>
						<select id="au-event" class="input" bind:value={draft.event} disabled={!!draft.id}>
							{#each EVENTS as [k, l] (k)}<option value={k}>{l}</option>{/each}
						</select>
					</div>
				{/if}
				{#if draft.kind !== 'schedule'}
					<div>
						<label class="label" for="au-stage">{draft.kind === 'robot' ? 'Runs in stage' : 'Moves the record to'}</label>
						<select id="au-stage" class="input" bind:value={draft.stage_id}>
							{#each category?.stages ?? [] as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
						</select>
					</div>
				{/if}
				<div class="sm:col-span-2">
					<label class="label" for="au-name">Name</label>
					<input id="au-name" class="input" maxlength="200" bind:value={draft.name} placeholder={draft.kind === 'trigger' ? EVENTS.find((e) => e[0] === (draft as TriggerDraft).event)?.[1] : ACTIONS.find((a) => a[0] === (draft as ActionDraft).action)?.[1]} />
				</div>
			</div>

			{#if draft.kind === 'schedule'}
				{@const d = draft}
				<div class="space-y-3 rounded-xl border border-slate-200 p-3">
					<div class="flex flex-wrap items-center gap-2">
						<span class="label mb-0">Runs</span>
						<select class="input w-32" bind:value={d.frequency} aria-label="Frequency"><option value="daily">every day</option><option value="weekly">every week</option><option value="monthly">every month</option></select>
						{#if d.frequency === 'monthly'}
							<span>on day</span><input class="input w-20" type="number" min="1" max="31" bind:value={d.month_day} aria-label="Day of the month" />
						{/if}
						<span>at</span><input class="input w-32" type="time" bind:value={d.at_time} aria-label="Time" />
						<select class="input w-52" bind:value={d.timezone} aria-label="Time zone">
							{#if !timezones.includes(d.timezone)}<option>{d.timezone}</option>{/if}
							{#each timezones as tz (tz)}<option>{tz}</option>{/each}
						</select>
					</div>
					{#if d.frequency === 'weekly'}
						<div class="flex flex-wrap gap-1.5" role="group" aria-label="Days of the week">
							{#each DAYS as day, i (day)}
								<button type="button" aria-pressed={d.weekdays.includes(i + 1)} class="rounded-full px-3 py-1 text-xs {d.weekdays.includes(i + 1) ? 'bg-brand-600 text-white' : 'bg-slate-100 text-slate-600 hover:bg-slate-200'}" onclick={() => (d.weekdays = d.weekdays.includes(i + 1) ? d.weekdays.filter((x) => x !== i + 1) : [...d.weekdays, i + 1].sort())}>{day}</button>
							{/each}
						</div>
					{/if}
					{#if d.frequency === 'monthly' && d.month_day > 28}<p class="text-xs text-slate-500">In shorter months it runs on the last day.</p>{/if}
					<div>
						<span class="label">For records in</span>
						<div class="flex flex-wrap gap-1.5">
							{#each category?.stages ?? [] as s (s.id)}
								<label class="flex items-center gap-1.5 rounded-full border border-slate-200 px-2.5 py-1 text-xs"><input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={d.stage_ids.includes(s.id)} onchange={(e) => (d.stage_ids = e.currentTarget.checked ? [...d.stage_ids, s.id] : d.stage_ids.filter((x) => x !== s.id))} />{s.name}</label>
							{/each}
						</div>
						{#if !d.stage_ids.length}<p class="mt-1 text-xs text-slate-500">None chosen: every open stage.</p>{/if}
					</div>
				</div>
			{/if}

			{#if draft.kind !== 'trigger'}
				{@render actionEditor()}
			{/if}

			{#if draft.kind === 'robot'}
				{@const d = draft}
				<div class="space-y-3 rounded-xl border border-slate-200 p-3">
					<div class="flex flex-wrap items-center gap-2">
						<span class="label mb-0">Run</span>
						<select class="input w-48" bind:value={d.when} aria-label="When">
							<option value="now">when the record arrives</option>
							<option value="delay">after a delay</option>
							<option value="field">relative to a date</option>
						</select>
						{#if d.when === 'delay'}
							{@render amountUnit('Delay', () => d.delay, (v) => (d.delay = v), () => d.unit, (v) => (d.unit = v))}
						{:else if d.when === 'field'}
							{@render amountUnit('Offset', () => d.offset, (v) => (d.offset = v), () => d.offsetUnit, (v) => (d.offsetUnit = v))}
							<select class="input w-24" bind:value={d.direction} aria-label="Before or after"><option value="before">before</option><option value="after">after</option></select>
							<select class="input w-48" bind:value={d.field} aria-label="Date field">{#each dateFields as f (f.code)}<option value={f.code}>{f.label}</option>{/each}</select>
						{/if}
					</div>
					{#if d.when === 'field'}
						<p class="text-xs text-slate-500">Dates without a time count from 09:00 in the responsible person's time zone. Changing the date reschedules the robot; a time already past runs at once.</p>
					{:else if d.when === 'delay'}
						<p class="text-xs text-slate-500">Skipped if the record has left the stage by then.</p>
					{/if}
					<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={d.after_previous} /> Start only after the robot above it has finished</label>
					<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={d.waits} /> Wait before acting</label>
					{#if d.waits}
						<div class="space-y-3 pl-6">
							<div class="flex flex-wrap items-center gap-2">
								<span>until</span>
								<select class="input w-60" bind:value={d.wait.until} aria-label="Wait until">{#each WAITS as [k, l] (k)}<option value={k}>{l}</option>{/each}</select>
								<span>for at most</span>
								{@render amountUnit('Timeout', () => d.waitAmount, (v) => (d.waitAmount = v), () => d.waitUnit, (v) => (d.waitUnit = v))}
							</div>
							{#if d.wait.until === 'conditions'}
								{@render conditionList(d.wait.conditions, 'Wait for', 'checked whenever the record changes')}
							{/if}
							<div class="flex flex-wrap gap-4">
								<span>Then, if it never happens:</span>
								<label class="flex items-center gap-1.5"><input type="radio" class="text-brand-600" value="run" bind:group={d.wait.on_timeout} /> run anyway</label>
								<label class="flex items-center gap-1.5"><input type="radio" class="text-brand-600" value="skip" bind:group={d.wait.on_timeout} /> skip it</label>
							</div>
						</div>
					{/if}
				</div>
			{:else if draft.kind === 'trigger'}
				{#if draft.event === 'activity_done'}
					<div>
						<label class="label" for="au-akind">Activity type</label>
						<select id="au-akind" class="input" value={draft.params.kind ?? ''} onchange={(e) => (draft!.params.kind = e.currentTarget.value || undefined)}>
							<option value="">Any</option><option value="call">Call</option><option value="meeting">Meeting</option><option value="todo">To-do</option><option value="email">Email</option>
						</select>
					</div>
				{:else if draft.event === 'document_created'}
					<div>
						<label class="label" for="au-dtpl">Template</label>
						<select id="au-dtpl" class="input" value={draft.params.template_id ?? ''} onchange={(e) => (draft!.params.template_id = e.currentTarget.value || undefined)}>
							<option value="">Any</option>
							{#each typeTemplates as t (t.id)}<option value={t.id}>{t.name}</option>{/each}
						</select>
					</div>
				{:else if draft.event === 'signature_completed' || draft.event === 'signature_declined'}
					<div>
						<label class="label" for="au-sgt">Signing template</label>
						<select id="au-sgt" class="input" value={draft.params.sign_template_id ?? ''} onchange={(e) => (draft!.params.sign_template_id = e.currentTarget.value || undefined)}>
							<option value="">Any</option>
							{#each typeSignTemplates as t (t.id)}<option value={t.id}>{t.name}</option>{/each}
						</select>
					</div>
				{:else if draft.event === 'webhook'}
					{#if draft.token}
						<div class="rounded-xl bg-slate-50 p-3 text-xs">
							<p class="mb-1 font-medium text-slate-800">Webhook URL (keep it secret)</p>
							<code class="block break-all text-slate-700">{webhookBase}{draft.token}</code>
							<p class="mt-2 text-slate-500">POST <code>{'{"item_id": "<record id>"}'}</code> as JSON to move that record here.</p>
							<button type="button" class="btn btn-ghost mt-1 px-2 py-1 text-xs" onclick={() => navigator.clipboard?.writeText(webhookBase + (draft as TriggerDraft).token).then(() => toasts.success('Copied'))}>Copy URL</button>
						</div>
					{:else}
						<p class="text-xs text-slate-500">A secret URL is created when you save.</p>
					{/if}
				{/if}
				<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.allow_backwards} /> Also move records back from later stages</label>
			{/if}

			{@render conditionList(draft.conditions, 'Conditions', draft.kind === 'schedule' ? 'records must match all' : 'all must hold')}
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.is_active} /> Active</label>
		</div>
	{/if}
	{#snippet footer()}
		{#if draft?.id}<button class="btn btn-ghost mr-auto text-red-600" onclick={remove}><Icon name="trash" class="size-4" /> Delete</button>{/if}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
	{/snippet}
</Modal>

<Modal bind:open={reqOpen} title="Required in: {req?.name ?? ''}" size="md">
	{#if req}
		{@const r = req}
		<div class="space-y-3 text-sm">
			<p class="text-slate-600">A record can only move into this stage once these are filled in.</p>
			<div class="grid max-h-80 grid-cols-2 gap-x-4 gap-y-1.5 overflow-y-auto">
				{#each requirable as f (f.code)}
					<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" value={f.code} bind:group={r.fields} /> {f.label}</label>
				{/each}
			</div>
		</div>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (reqOpen = false)}>Cancel</button>
		<button class="btn btn-primary" onclick={saveRequired}>Save</button>
	{/snippet}
</Modal>

<Modal bind:open={slaOpen} title="Time limit: {sla?.name ?? ''}" size="md">
	{#if sla}
		{@const s = sla}
		<div class="space-y-4 text-sm">
			<p class="text-slate-600">Records that stay in this stage longer are flagged on the board and in their timeline, and their responsible person is notified.</p>
			<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={s.on} /> Limit the time in this stage</label>
			{#if s.on}
				<div class="flex items-center gap-2 pl-6">
					<span>at most</span>
					{@render amountUnit('Time limit', () => s.amount, (v) => (s.amount = v), () => s.unit, (v) => (s.unit = v))}
				</div>
				<label class="flex items-center gap-2 pl-6"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={s.escalate} /> Also notify the heads of their departments</label>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (slaOpen = false)}>Cancel</button>
		<button class="btn btn-primary" onclick={saveSla}>Save</button>
	{/snippet}
</Modal>
