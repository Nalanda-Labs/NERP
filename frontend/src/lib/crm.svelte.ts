// CRM types, the cached configuration, and locale-aware formatting for any country.

import { api } from './api.ts';
import { realtime } from './realtime.svelte.ts';

export type Scope = 'none' | 'own' | 'department' | 'subdepartments' | 'all';

export interface CrmStage {
	id: string;
	category_id: string;
	name: string;
	color: string;
	sort: number;
	semantics: 'process' | 'success' | 'failure';
	/** Time limit for records in this stage, in minutes. */
	sla_minutes: number | null;
	sla_escalate: boolean;
	/** Fields that must be filled in before a record enters this stage. */
	required_fields?: string[];
}
export interface CrmCategory {
	id: string;
	name: string;
	sort: number;
	is_default: boolean;
	stages: CrmStage[];
}
export interface SystemField {
	code: string;
	label: string;
	kind: string;
}
export interface FieldDef {
	id: string;
	code: string;
	label: string;
	kind: string;
	is_multiple: boolean;
	is_required: boolean;
	show_in_list: boolean;
	settings: { options?: { id: string; label: string }[]; types?: string[] };
	sort: number;
	/** Who sees the field (CRM admins only get these); both empty: everyone. */
	visible_user_ids?: string[];
	visible_department_ids?: string[];
}
export interface CrmType {
	id: number;
	/** lead | deal | contact | company, or a smart process's code. */
	code: string;
	name: string;
	name_plural: string;
	has_pipelines: boolean;
	has_stages: boolean;
	has_amount: boolean;
	/** Linked to a company and contacts. */
	has_client: boolean;
	/** Built in rather than a smart process. */
	is_system: boolean;
	/** 'quote', 'invoice' or 'order' for those built-in types (they run like smart processes). */
	kind: 'quote' | 'invoice' | 'order' | null;
	icon: string;
	categories: CrmCategory[];
	system_fields: SystemField[];
	custom_fields: FieldDef[];
	rights: Record<'read' | 'add' | 'write' | 'delete' | 'export' | 'import', Scope>;
}
export interface CrmConfig {
	types: CrmType[];
	currencies: { code: string; rate: string; is_base: boolean }[];
	base_currency: string;
	options: Record<string, string[]>;
	/** Names of the company's own traffic sources, by code. */
	source_labels?: Record<string, string>;
	is_admin: boolean;
	can_manage_products: boolean;
}

export interface Multifield {
	id?: string;
	kind: 'phone' | 'email' | 'web' | 'im';
	value_type: string;
	value: string;
}
export interface Address {
	line1?: string;
	line2?: string;
	city?: string;
	region?: string;
	postal_code?: string;
	country?: string;
}
export interface CrmRow {
	id: string;
	entity_type: number;
	number: number;
	title: string;
	category_id: string | null;
	stage_id: string | null;
	stage_name: string | null;
	stage_color: string | null;
	stage_semantics: 'process' | 'success' | 'failure';
	assigned_to: string | null;
	assigned_name: string | null;
	assigned_avatar_file_id: string | null;
	honorific: string | null;
	first_name: string | null;
	middle_name: string | null;
	last_name: string | null;
	job_title: string | null;
	birthdate: string | null;
	company_name: string | null;
	company_kind: string | null;
	industry: string | null;
	employees: string | null;
	annual_revenue: string | null;
	opportunity: string;
	currency: string | null;
	opportunity_base: string;
	is_manual_amount: boolean;
	probability: number | null;
	close_date: string | null;
	source: string | null;
	source_details: string | null;
	address: Address | null;
	comments: string | null;
	company_id: string | null;
	company_title: string | null;
	source_item_id: string | null;
	uf: Record<string, unknown>;
	utm: Record<string, string> | null;
	is_closed: boolean;
	closed_at: string | null;
	stage_changed_at: string;
	last_activity_at: string;
	position: number;
	open_activities: number;
	next_activity_at: string | null;
	created_at: string;
	updated_at: string;
	phone: string | null;
	email: string | null;
}
export interface Linked {
	id: string;
	entity_type: number;
	number: number;
	title: string;
	stage_name: string | null;
	stage_color: string | null;
	opportunity: string;
	currency: string | null;
	stage_semantics: 'process' | 'success' | 'failure' | null;
	close_date: string | null;
	is_primary: boolean;
}
export interface CrmDetail extends CrmRow {
	multifields: Multifield[];
	contacts: Linked[];
	related: Linked[];
	source_item: Linked | null;
	/** Records made from this one: quotes and invoices of a deal, records made by robots. */
	children: Linked[];
	/** Employees following the record. */
	observers: { id: string; full_name: string; avatar_file_id: string | null }[];
	can_write: boolean;
	can_delete: boolean;
}
export interface Activity {
	id: string;
	item_id: string;
	item_type: number;
	item_title: string;
	kind: 'todo' | 'call' | 'meeting' | 'email';
	subject: string;
	description: string | null;
	direction: 'incoming' | 'outgoing' | null;
	starts_at: string | null;
	ends_at: string | null;
	deadline: string | null;
	responsible_id: string;
	responsible_name: string;
	is_completed: boolean;
	completed_at: string | null;
	result: string | null;
	is_overdue: boolean;
	created_at: string;
}

const BUILTIN_CODES: Record<number, string> = { 1: 'lead', 2: 'deal', 3: 'contact', 4: 'company' };

/** URL code of a record type by id (smart processes come from the loaded config). */
export function typeCode(id: number): string {
	return BUILTIN_CODES[id] ?? crm.config?.types.find((t) => t.id === id)?.code ?? 'deal';
}

/** Singular name of a record type by id. */
export function typeName(id: number): string {
	return crm.config?.types.find((t) => t.id === id)?.name ?? BUILTIN_CODES[id] ?? 'Record';
}

/** Whether records of this type link to a company and contacts. */
export function linksCompany(t: CrmType): boolean {
	return t.is_system ? t.code !== 'company' : t.has_client;
}
export function linksContacts(t: CrmType): boolean {
	return t.is_system ? t.code !== 'contact' : t.has_client;
}

// ---------------------------------------------------------------------------
// Config store
// ---------------------------------------------------------------------------

class CrmStore {
	config = $state<CrmConfig | null>(null);
	counters = $state<{ types: Record<string, { overdue: number; today: number; idle: number; attention: number }>; activities_overdue: number; activities_today: number } | null>(null);
	private loading: Promise<CrmConfig> | null = null;
	private listening = false;

	async load(force = false): Promise<CrmConfig> {
		if (this.config && !force) return this.config;
		this.loading ??= api.get<CrmConfig>('/crm/config').then((c) => {
			this.config = c;
			this.loading = null;
			return c;
		});
		return this.loading;
	}

	type(code: string): CrmType | undefined {
		return this.config?.types.find((t) => t.code === code);
	}

	async loadCounters() {
		this.counters = await api.get('/crm/counters');
		if (!this.listening) {
			this.listening = true;
			let t: ReturnType<typeof setTimeout>;
			realtime.on('crm.changed', () => {
				clearTimeout(t);
				t = setTimeout(() => this.loadCounters(), 800);
			});
		}
	}
}
export const crm = new CrmStore();

// ---------------------------------------------------------------------------
// Formatting (locale-aware, every country/currency)
// ---------------------------------------------------------------------------

const locale = () => (typeof navigator !== 'undefined' ? navigator.language : 'en');

export function money(amount: string | number | null | undefined, currency: string | null | undefined): string {
	const n = typeof amount === 'string' ? Number(amount) : (amount ?? 0);
	if (!currency) return n.toLocaleString(locale());
	try {
		return new Intl.NumberFormat(locale(), { style: 'currency', currency, maximumFractionDigits: 2 }).format(n);
	} catch {
		return `${n.toLocaleString(locale())} ${currency}`;
	}
}

/** Compact money for board headers: "€1.2M". */
export function moneyShort(amount: string | number, currency: string): string {
	try {
		return new Intl.NumberFormat(locale(), { style: 'currency', currency, notation: 'compact', maximumFractionDigits: 1 }).format(Number(amount));
	} catch {
		return money(amount, currency);
	}
}

let regionNames: Intl.DisplayNames | null = null;
let currencyNames: Intl.DisplayNames | null = null;
export function countryName(code: string | null | undefined): string {
	if (!code) return '';
	try {
		regionNames ??= new Intl.DisplayNames([locale()], { type: 'region' });
		return regionNames.of(code) ?? code;
	} catch {
		return code;
	}
}
export function currencyName(code: string): string {
	try {
		currencyNames ??= new Intl.DisplayNames([locale()], { type: 'currency' });
		return currencyNames.of(code) ?? code;
	} catch {
		return code;
	}
}

/** Every ISO 3166 country the browser knows, sorted by localized name. */
export function allCountries(): { code: string; name: string }[] {
	const codes: string[] = [];
	for (let a = 65; a <= 90; a++)
		for (let b = 65; b <= 90; b++) {
			const c = String.fromCharCode(a, b);
			const n = countryName(c);
			if (n && n !== c && !/^[A-Z]{2}$/.test(n)) codes.push(c);
		}
	const blocked = new Set(['EU', 'EZ', 'UN', 'QO', 'XA', 'XB', 'ZZ', 'AC', 'CP', 'DG', 'EA', 'IC', 'TA']);
	return codes.filter((c) => !blocked.has(c)).map((code) => ({ code, name: countryName(code) })).sort((x, y) => x.name.localeCompare(y.name));
}

/** Every ISO 4217 currency the browser knows. */
export function allCurrencies(): { code: string; name: string }[] {
	const codes: string[] = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.('currency') ?? ['USD', 'EUR'];
	return codes.map((code) => ({ code, name: currencyName(code) }));
}

export function formatAddress(a: Address | null | undefined): string {
	if (!a) return '';
	return [a.line1, a.line2, [a.postal_code, a.city].filter(Boolean).join(' '), a.region, countryName(a.country)].filter(Boolean).join(', ');
}

/** "web_form" → "Web form". */
export function optionLabel(v: string | null | undefined): string {
	if (!v) return '';
	const own = crm.config?.source_labels?.[v];
	if (own) return own;
	const special: Record<string, string> = { it: 'IT', x: 'X (Twitter)', im: 'Messenger', web_form: 'Web form', real_estate: 'Real estate' };
	return special[v] ?? v.charAt(0).toUpperCase() + v.slice(1).replace(/_/g, ' ');
}

export function itemLink(type: number | string, id: string): string {
	return `/crm/${typeof type === 'number' ? typeCode(type) : type}/${id}`;
}

/** Text shown for a custom field value. */
export function ufText(def: FieldDef, v: unknown): string {
	if (v === null || v === undefined || v === '') return '';
	const one = (x: unknown): string => {
		switch (def.kind) {
			case 'boolean':
				return x ? 'Yes' : 'No';
			case 'list':
				return def.settings.options?.find((o) => o.id === x)?.label ?? String(x);
			case 'money': {
				const m = x as { amount: string; currency: string };
				return money(m.amount, m.currency);
			}
			case 'date':
				return new Date(String(x) + 'T00:00:00').toLocaleDateString();
			case 'datetime':
				return new Date(String(x)).toLocaleString();
			case 'address':
				return formatAddress(x as Address);
			default:
				return String(x);
		}
	};
	return Array.isArray(v) ? v.map(one).join(', ') : one(v);
}

/** 90 → "1 h 30 min", 2880 → "2 d". */
export function durationText(minutes: number): string {
	const m = Math.round(minutes);
	const d = Math.floor(m / 1440), h = Math.floor((m % 1440) / 60), r = m % 60;
	if (!d && !h) return `${r} min`;
	if (!d) return r ? `${h} h ${r} min` : `${h} h`;
	return h ? `${d} d ${h} h` : `${d} d`;
}

/** Minutes a record has been over its stage's time limit (null: not over, or no limit). */
export function overLimit(stage: CrmStage | undefined, stageChangedAt: string, now = Date.now()): number | null {
	if (!stage?.sla_minutes || stage.semantics !== 'process') return null;
	const over = (now - new Date(stageChangedAt).getTime()) / 60000 - stage.sla_minutes;
	return over > 0 ? over : null;
}

/** An unpaid invoice past its due date (dates as YYYY-MM-DD, compared in local time). */
export function invoiceOverdue(t: { kind?: string | null } | undefined, r: { close_date: string | null; is_closed?: boolean; stage_semantics?: string | null }): boolean {
	if (t?.kind !== 'invoice' || !r.close_date || r.is_closed || (r.stage_semantics && r.stage_semantics !== 'process')) return false;
	const today = new Date();
	const local = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, '0')}-${String(today.getDate()).padStart(2, '0')}`;
	return r.close_date < local;
}

// ---------------------------------------------------------------------------
// Record card layouts
// ---------------------------------------------------------------------------

export interface CardSection {
	id: string;
	title: string;
	fields: string[];
}
export interface CardLayout {
	sections: CardSection[];
	hidden: string[];
	hide_empty: boolean;
}
export interface CardField {
	code: string;
	label: string;
	required: boolean;
}
export interface CardLayoutInfo {
	layout: CardLayout;
	/** Whose layout this is: the viewer's own, the shared one, or the built-in default. */
	scope: 'mine' | 'common' | 'default';
	has_common: boolean;
	fields: CardField[];
	can_edit_common: boolean;
}

const cardCache = new Map<string, Promise<CardLayoutInfo>>();
/** The card layout of a record type for the signed-in user (cached until it changes). */
export function cardLayout(code: string, fresh = false): Promise<CardLayoutInfo> {
	if (fresh || !cardCache.has(code)) {
		const p = api.get<CardLayoutInfo>(`/crm/${code}/card-layout`);
		p.catch(() => cardCache.delete(code));
		cardCache.set(code, p);
	}
	return cardCache.get(code)!;
}
export function setCardLayout(code: string, info: CardLayoutInfo) {
	cardCache.set(code, Promise.resolve(info));
}
