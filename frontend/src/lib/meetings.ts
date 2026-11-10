// Meetings: types and words shared by the meeting pages.

export type MeetingState = 'prepare' | 'in_progress' | 'closed';
export type Role = 'owner' | 'keeper' | 'member';

export interface Person { id: string; name: string; avatar_file_id: string | null }
export interface FileRef { id: string; name: string; size: number }

export interface MeetingSummary {
	id: string;
	title: string;
	starts_at: string;
	duration_minutes: number;
	state: MeetingState;
	place: string | null;
	project: { id: string; name: string } | null;
	my_role: Role | null;
	people: number;
	topics: number;
}

export interface Report { user_id: string; name: string; body: string; updated_at: string; files: FileRef[] }

export interface AgendaItem {
	id: string;
	topic_id: string;
	parent_id: string | null;
	sort: number;
	title: string;
	description: string | null;
	responsible: Person[];
	duration_minutes: number | null;
	decision: string | null;
	status: 'open' | 'done' | 'carried';
	proposed_by: Person | null;
	task: { id: string; number: number; title: string; status: string } | null;
	earlier: number;
	reports: Report[];
}

export interface Meeting {
	id: string;
	title: string;
	description: string | null;
	place: string | null;
	starts_at: string;
	ends_at: string;
	duration_minutes: number;
	timezone: string;
	state: MeetingState;
	project_id: string | null;
	project: { id: string; name: string } | null;
	parent_id: string | null;
	previous: { id: string; title: string; starts_at: string } | null;
	next: { id: string; title: string; starts_at: string } | null;
	minutes: string | null;
	event_id: string | null;
	files: FileRef[];
	started_at: string | null;
	closed_at: string | null;
	members: (Person & { role: Role; attended: boolean | null })[];
	agenda: AgendaItem[];
	my_role: Role | 'viewer';
	can: { run: boolean; propose: boolean; manage_people: boolean; delete: boolean };
}

export const STATE_LABEL: Record<MeetingState, string> = { prepare: 'Being prepared', in_progress: 'In progress', closed: 'Closed' };
export const STATE_TONE: Record<MeetingState, string> = {
	prepare: 'bg-sky-50 text-sky-700',
	in_progress: 'bg-amber-50 text-amber-800',
	closed: 'bg-slate-100 text-slate-600'
};
export const ROLE_LABEL: Record<Role, string> = { owner: 'Runs it', keeper: 'Writes the minutes', member: 'Takes part' };

export const meetingTime = (iso: string, minutes?: number) => {
	const s = new Date(iso);
	const day = s.toLocaleDateString([], { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' });
	const t = (d: Date) => d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	return minutes ? `${day}, ${t(s)}–${t(new Date(+s + minutes * 60000))}` : `${day}, ${t(s)}`;
};

/** A topic and its sub-points, in agenda order. */
export function tree(items: AgendaItem[]): (AgendaItem & { subs: AgendaItem[] })[] {
	return items.filter((i) => !i.parent_id).map((i) => ({ ...i, subs: items.filter((s) => s.parent_id === i.id) }));
}
