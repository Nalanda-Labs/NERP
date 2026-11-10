// Help desk: types and words shared by the help-desk pages.
import { api } from '#lib/api.ts';

export type DictKind = 'category' | 'criticality' | 'status' | 'mark' | 'difficulty';

export interface Entry {
	id: string;
	kind: DictKind;
	name: string;
	description: string | null;
	sort: number;
	is_default: boolean;
	responsible_id: string | null;
}

export interface Ticket {
	id: string;
	number: number;
	title: string;
	channel: 'staff' | 'portal' | 'email';
	requester_id: string | null;
	requester_name: string | null;
	requester_email: string | null;
	contact_id: string | null;
	category_id: string | null;
	criticality_id: string | null;
	status_id: string | null;
	difficulty_id?: string | null;
	mark_id: string | null;
	sla_id?: string | null;
	sla_name?: string | null;
	responsible_id: string | null;
	responsible_name: string | null;
	waiting_for: 'support' | 'customer';
	waiting_since: string | null;
	deadline_at: string | null;
	is_overdue: boolean;
	overdue_count?: number;
	on_hold: boolean;
	messages: number;
	last_message_at: string;
	first_response_at: string | null;
	minutes_spent?: number;
	is_spam?: boolean;
	closed_at: string | null;
	auto_closed: boolean;
	created_at: string;
}

export interface Message {
	id: string;
	kind: 'message' | 'note' | 'log';
	by_support: boolean;
	author_id: string | null;
	author_name: string;
	author_email: string | null;
	author_avatar_file_id: string | null;
	body: string;
	files: { id: string; name: string; size: number }[];
	minutes_spent: number;
	is_overdue: boolean;
	email_status: 'queued' | 'sent' | 'failed' | null;
	created_at: string;
}

export type TicketDetail = Omit<Ticket, 'messages'> & { messages: Message[]; as_support: boolean; can_rate: boolean };

export interface Sla {
	id: string;
	name: string;
	description: string | null;
	priority: number;
	is_default: boolean;
	response_minutes: number;
	notice_minutes: number;
	responsible_id: string | null;
	category_ids: string[];
	criticality_ids: string[];
	requesters: 'any' | 'staff' | 'external';
	hours: Record<string, [number, number][]>;
	skip_holidays: boolean;
}

export const KIND_LABEL: Record<DictKind, string> = {
	category: 'Categories',
	criticality: 'Criticality',
	status: 'Statuses',
	mark: 'Satisfaction marks',
	difficulty: 'Difficulty'
};

export const CHANNEL_LABEL: Record<Ticket['channel'], string> = { staff: 'In the app', portal: 'Help page', email: 'Email' };

export const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];

/** The dictionary, loaded once per page. */
export async function loadDictionary(): Promise<Entry[]> {
	return (await api.get<{ items: Entry[] }>('/support/dictionary')).items;
}

export const nameOf = (dict: Entry[], id: string | null | undefined) => (id ? (dict.find((e) => e.id === id)?.name ?? '') : '');

/** "2 h 30 min", "3 d 4 h" — for SLA times and averages. */
export function duration(minutes: number | null | undefined): string {
	if (minutes === null || minutes === undefined) return '—';
	const m = Math.round(Math.abs(minutes));
	if (m < 60) return `${m} min`;
	if (m < 60 * 24) return `${Math.floor(m / 60)} h${m % 60 ? ` ${m % 60} min` : ''}`;
	const d = Math.floor(m / 1440), h = Math.floor((m % 1440) / 60);
	return `${d} d${h ? ` ${h} h` : ''}`;
}

/** How long until a deadline (negative: overdue), for lists. */
export function dueIn(deadline: string | null, now = Date.now()): { text: string; late: boolean } | null {
	if (!deadline) return null;
	const mins = (Date.parse(deadline) - now) / 60000;
	return mins < 0 ? { text: `${duration(-mins)} overdue`, late: true } : { text: `due in ${duration(mins)}`, late: false };
}

export const hhmm = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
export const fromHhmm = (s: string) => {
	const [h, m] = s.split(':').map(Number);
	return (h || 0) * 60 + (m || 0);
};
