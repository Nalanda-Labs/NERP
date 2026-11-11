// BI: shapes shared by the dashboard pages, the chart editor and the public page.

export type ColType = 'text' | 'number' | 'date' | 'time' | 'bool';
export type Col = { key: string; label: string; type: ColType };
export type Filter = { field: string; op: string; value?: unknown };
export type Spec = {
	dataset: string;
	groups?: { field: string; bucket?: string }[];
	measures?: { agg: string; field?: string; label?: string }[];
	columns?: string[];
	filters?: Filter[];
	date_field?: string | null;
	sort?: { by: string; desc: boolean } | null;
	limit?: number | null;
	compare?: boolean;
};
export type Widget = {
	id: string;
	title: string;
	chart: string;
	w: number;
	h: number;
	compare: boolean;
	spec?: Spec;
};
export type Result = {
	columns: Col[];
	rows: unknown[][];
	previous?: { rows: unknown[][]; from: string; to: string };
};
export type Figures = { id: string; result?: Result; error?: string };
export type Period = { preset?: string; from?: string; to?: string };
export type DashFilters = { period?: Period; filters?: Filter[] };
export type Dataset = {
	key: string;
	id?: string;
	name: string;
	group: string;
	kind: string;
	columns: Col[];
	row_count?: number;
	refreshed_at?: string | null;
	last_error?: string | null;
	url?: string | null;
	refresh_hours?: number | null;
	source_id?: string | null;
	source_name?: string | null;
	query?: string | null;
};

export const CHARTS: [string, string][] = [
	['number', 'Single figure'],
	['bar', 'Columns'],
	['hbar', 'Bars'],
	['line', 'Line'],
	['area', 'Area'],
	['pie', 'Pie'],
	['funnel', 'Funnel'],
	['table', 'Table']
];

export const PERIODS: [string, string][] = [
	['all', 'All time'],
	['today', 'Today'],
	['yesterday', 'Yesterday'],
	['this_week', 'This week'],
	['last_week', 'Last week'],
	['this_month', 'This month'],
	['last_month', 'Last month'],
	['this_quarter', 'This quarter'],
	['last_quarter', 'Last quarter'],
	['this_year', 'This year'],
	['last_year', 'Last year'],
	['last_7_days', 'Last 7 days'],
	['last_30_days', 'Last 30 days'],
	['last_90_days', 'Last 90 days'],
	['last_365_days', 'Last 365 days']
];

export const AGGS: [string, string][] = [
	['count', 'Count of rows'],
	['sum', 'Total of'],
	['avg', 'Average of'],
	['min', 'Smallest'],
	['max', 'Largest'],
	['count_distinct', 'Different values of']
];

export const BUCKETS: [string, string][] = [
	['day', 'by day'],
	['week', 'by week'],
	['month', 'by month'],
	['quarter', 'by quarter'],
	['year', 'by year']
];

/** Filter conditions that fit a column type. */
export function opsFor(t: ColType): [string, string][] {
	const empty: [string, string][] = [
		['empty', 'is empty'],
		['not_empty', 'is not empty']
	];
	if (t === 'text')
		return [
			['eq', 'is'],
			['ne', 'is not'],
			['contains', 'contains'],
			['not_contains', 'does not contain'],
			['in', 'is one of'],
			...empty
		];
	if (t === 'bool') return [['eq', 'is'], ...empty];
	if (t === 'number')
		return [
			['eq', '='],
			['ne', '≠'],
			['gt', '>'],
			['gte', '≥'],
			['lt', '<'],
			['lte', '≤'],
			['between', 'between'],
			...empty
		];
	return [
		['eq', 'on'],
		['gte', 'on or after'],
		['lte', 'on or before'],
		['between', 'between'],
		...empty
	];
}

export const needsValue = (op: string) => op !== 'empty' && op !== 'not_empty';

/** Conditions filled in far enough to apply (half-made ones wait). */
export const complete = (fs: Filter[]): Filter[] =>
	fs.filter(
		(f) =>
			!needsValue(f.op) ||
			(Array.isArray(f.value)
				? f.value.length > 0 && f.value.every((v) => v !== '')
				: f.value !== '' && f.value !== undefined && f.value !== null)
	);

export function periodLabel(p: Period | undefined): string {
	if (!p || (!p.preset && !p.from)) return 'All time';
	if (p.from) return `${p.from} – ${p.to}`;
	return PERIODS.find(([k]) => k === p.preset)?.[1] ?? p.preset!;
}

const nf = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });
export function show(v: unknown, t: ColType): string {
	if (v === null || v === undefined || v === '') return '—';
	if (t === 'number') return nf.format(Number(v));
	if (t === 'bool') return v ? 'Yes' : 'No';
	if (t === 'date') return new Date(`${String(v).slice(0, 10)}T00:00:00`).toLocaleDateString();
	if (t === 'time') return new Date(String(v)).toLocaleString();
	return String(v);
}

export function filterLabel(f: Filter, cols: Col[]): string {
	const c = cols.find((x) => x.key === f.field);
	const op = c ? opsFor(c.type).find(([k]) => k === f.op)?.[1] : f.op;
	const v = Array.isArray(f.value)
		? f.value.join(f.op === 'between' ? ' and ' : ', ')
		: f.value === true
			? 'yes'
			: f.value === false
				? 'no'
				: f.value;
	return `${c?.label ?? f.field} ${op}${needsValue(f.op) ? ` ${v}` : ''}`;
}
