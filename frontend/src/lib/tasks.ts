export type TaskStatus = 'new' | 'in_progress' | 'review' | 'done' | 'deferred';
export type Priority = 'low' | 'normal' | 'high';

export interface TaskSummary {
	id: string;
	number: number;
	title: string;
	status: TaskStatus;
	priority: Priority;
	deadline: string | null;
	is_overdue: boolean;
	project_id: string | null;
	project_name: string | null;
	project_color: string | null;
	parent_id: string | null;
	responsible_id: string;
	responsible_name: string;
	responsible_avatar_file_id: string | null;
	created_by: string;
	creator_name: string;
	creator_avatar_file_id: string | null;
	requires_review: boolean;
	position: number;
	start_date: string | null;
	stage_id: string | null;
	template_id: string | null;
	blocked_by_count: number;
	checklist_total: number;
	checklist_done: number;
	comment_count: number;
	subtask_count: number;
	file_count: number;
	time_spent_secs: number;
	completed_at: string | null;
	last_activity_at: string;
	created_at: string;
	/** Scrum teams: estimate, epic, sprint (null: in the backlog), rank. */
	story_points: number | null;
	epic_id: string | null;
	epic_name: string | null;
	epic_color: string | null;
	sprint_id: string | null;
	backlog_sort: number;
	rating: 'good' | 'bad' | null;
	result_comment_id: string | null;
	requires_result: boolean;
	skip_nonworking: boolean;
	/** Comments by others since I last opened it (lists). */
	new_comments: number;
}

export interface TaskPerson {
	user_id: string;
	full_name: string;
	avatar_file_id: string | null;
	position: string | null;
	role: 'creator' | 'responsible' | 'assistant' | 'watcher';
}

export interface ChecklistItem {
	id: string;
	title: string;
	is_done: boolean;
	sort: number;
	done_by: string | null;
	done_at: string | null;
	assignee_id: string | null;
	assignee_name: string | null;
}

export interface Attachment {
	id: string;
	original_name: string;
	mime_type: string;
	size_bytes: number;
	added_by: string | null;
	added_at: string;
}

export interface TaskAccess {
	view: boolean;
	edit: boolean;
	participate: boolean;
	delete: boolean;
	is_creator: boolean;
	is_responsible: boolean;
	is_assistant: boolean;
	is_watcher: boolean;
}

export interface TaskDetail extends TaskSummary {
	description: string | null;
	time_estimate_secs: number | null;
	people: TaskPerson[];
	checklist: ChecklistItem[];
	files: Attachment[];
	subtasks: TaskSummary[];
	parent: { id: string; number: number; title: string; status: TaskStatus } | null;
	access: TaskAccess;
	my_timer_started_at: string | null;
	blocked_by: DepRef[];
	blocking: DepRef[];
	template: { id: string; title: string; schedule: string | null; mine: boolean } | null;
	/** CRM records the task is about (those the viewer may open). */
	crm: { id: string; entity_type: number; code: string; type_name: string; number: number; title: string }[];
	fields: TaskField[];
	result: { comment_id: string; body: string; author_name: string | null; created_at: string } | null;
}

export type TaskFieldKind = 'text' | 'number' | 'date' | 'select' | 'bool' | 'user' | 'url';
export interface TaskFieldDef {
	id: string;
	project_id: string | null;
	project_name: string | null;
	name: string;
	kind: TaskFieldKind;
	options: string[];
	required: boolean;
	show_in_list: boolean;
	sort: number;
}
export interface TaskField extends TaskFieldDef {
	value: string | number | boolean | null;
}
export const FIELD_KINDS: Record<TaskFieldKind, string> = {
	text: 'Text',
	number: 'Number',
	date: 'Date',
	select: 'List',
	bool: 'Yes / no',
	user: 'Employee',
	url: 'Link'
};

export interface DepRef {
	id: string;
	number: number;
	title: string;
	status: TaskStatus;
	deadline: string | null;
}

export interface Stage {
	id: string;
	name: string;
	color: string;
	sort: number;
	status: TaskStatus | null;
}

export interface Recurrence {
	freq: 'daily' | 'weekly' | 'monthly';
	interval: number;
	weekdays: number[];
	month_day: number | null;
	time: string;
	timezone: string;
	starts_on: string;
	ends_on: string | null;
	max_runs: number | null;
}

export interface TaskTemplate {
	id: string;
	title: string;
	description: string | null;
	responsible_id: string | null;
	responsible_name: string | null;
	assistant_ids: string[];
	watcher_ids: string[];
	priority: Priority;
	project_id: string | null;
	project_name: string | null;
	requires_review: boolean;
	time_estimate_secs: number | null;
	checklist: string[];
	deadline_offset_secs: number | null;
	recurrence: Recurrence | null;
	next_run_at: string | null;
	last_run_at: string | null;
	run_count: number;
	is_active: boolean;
	created_by: string;
	created_by_name: string | null;
	subtasks: { title: string; responsible_id: string | null; deadline_offset_secs: number | null }[];
	shared: boolean;
	created_at: string;
	schedule: string | null;
	task_count: number;
}

export interface TaskComment {
	id: string;
	task_id: string;
	author_id: string | null;
	author_name: string | null;
	author_avatar_file_id: string | null;
	body: string;
	edited_at: string | null;
	created_at: string;
}

export interface TimeEntry {
	id: string;
	user_id: string;
	user_name: string;
	started_at: string;
	stopped_at: string | null;
	seconds: number;
	comment: string | null;
}

export interface HistoryEntry {
	id: string;
	actor_id: string | null;
	actor_name: string | null;
	field: string;
	old_value: string | null;
	new_value: string | null;
	created_at: string;
}

export interface ProjectSummary {
	id: string;
	name: string;
	description: string | null;
	visibility: 'open' | 'closed' | 'private';
	color: string;
	is_archived: boolean;
	member_count: number;
	open_task_count: number;
	overdue_task_count: number;
	my_role: 'owner' | 'member' | 'viewer' | null;
	created_at: string;
	/** group: an ongoing workgroup; project: with start and end dates; scrum: a Scrum team. */
	kind: 'group' | 'project' | 'scrum';
	starts_on: string | null;
	ends_on: string | null;
	/** I asked to join (closed groups). */
	requested: boolean;
	request_count: number;
}

export const KIND_LABEL: Record<ProjectSummary['kind'], string> = { group: 'Workgroup', project: 'Project', scrum: 'Scrum team' };
export const KIND_NOUN: Record<ProjectSummary['kind'], string> = { group: 'workgroup', project: 'project', scrum: 'Scrum team' };

export const VISIBILITY: Record<ProjectSummary['visibility'], { label: string; hint: string }> = {
	open: { label: 'Open', hint: 'Everyone sees it and can join' },
	closed: { label: 'Closed', hint: 'Listed; people ask to join, members see the content' },
	private: { label: 'Private', hint: 'Only members know it exists' }
};

export interface ProjectMember {
	user_id: string;
	full_name: string;
	position: string | null;
	avatar_file_id: string | null;
	role: 'owner' | 'member' | 'viewer';
}

export interface ProjectDetail extends ProjectSummary {
	members: ProjectMember[];
	stages: Stage[];
	access: { view: boolean; manage: boolean; contribute: boolean; role: string | null };
}

export interface TaskCounters {
	my_open: number;
	my_overdue: number;
	my_due_soon: number;
	awaiting_my_review: number;
	created_open: number;
}

export const STATUS: Record<TaskStatus, { label: string; badge: string; dot: string }> = {
	new: { label: 'To do', badge: 'bg-slate-100 text-slate-700', dot: 'bg-slate-400' },
	in_progress: { label: 'In progress', badge: 'bg-sky-50 text-sky-700', dot: 'bg-sky-500' },
	review: { label: 'Awaiting review', badge: 'bg-amber-50 text-amber-700', dot: 'bg-amber-500' },
	done: { label: 'Done', badge: 'bg-emerald-50 text-emerald-700', dot: 'bg-emerald-500' },
	deferred: { label: 'Deferred', badge: 'bg-slate-100 text-slate-500', dot: 'bg-slate-300' }
};

export const BOARD_COLUMNS: TaskStatus[] = ['new', 'in_progress', 'review', 'done'];

export const PRIORITY: Record<Priority, { label: string; class: string }> = {
	high: { label: 'High', class: 'text-red-600' },
	normal: { label: 'Normal', class: 'text-slate-500' },
	low: { label: 'Low', class: 'text-slate-400' }
};

/** Tailwind classes for each project colour (kept literal so Tailwind generates them). */
export const PROJECT_COLORS: Record<string, { bg: string; text: string; ring: string }> = {
	indigo: { bg: 'bg-indigo-500', text: 'text-indigo-700', ring: 'ring-indigo-200' },
	sky: { bg: 'bg-sky-500', text: 'text-sky-700', ring: 'ring-sky-200' },
	emerald: { bg: 'bg-emerald-500', text: 'text-emerald-700', ring: 'ring-emerald-200' },
	amber: { bg: 'bg-amber-500', text: 'text-amber-700', ring: 'ring-amber-200' },
	rose: { bg: 'bg-rose-500', text: 'text-rose-700', ring: 'ring-rose-200' },
	violet: { bg: 'bg-violet-500', text: 'text-violet-700', ring: 'ring-violet-200' },
	teal: { bg: 'bg-teal-500', text: 'text-teal-700', ring: 'ring-teal-200' },
	orange: { bg: 'bg-orange-500', text: 'text-orange-700', ring: 'ring-orange-200' },
	slate: { bg: 'bg-slate-500', text: 'text-slate-700', ring: 'ring-slate-200' }
};

export function formatDuration(secs: number): string {
	if (!secs) return '0m';
	const h = Math.floor(secs / 3600);
	const m = Math.floor((secs % 3600) / 60);
	if (h && m) return `${h}h ${m}m`;
	if (h) return `${h}h`;
	return `${Math.max(1, m)}m`;
}

export function formatClock(secs: number): string {
	const h = Math.floor(secs / 3600);
	const m = Math.floor((secs % 3600) / 60);
	const s = Math.floor(secs % 60);
	return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
}

/** "Today 17:00", "Tomorrow", "Mon 12 Nov" style deadline text. */
export function formatDeadline(iso: string | null): string {
	if (!iso) return 'No deadline';
	const d = new Date(iso);
	const now = new Date();
	const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
	const diff = Math.round((day(d) - day(now)) / 864e5);
	const time = d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
	if (diff === 0) return `Today ${time}`;
	if (diff === 1) return `Tomorrow ${time}`;
	if (diff === -1) return `Yesterday ${time}`;
	return d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric' });
}

/** Value for <input type="datetime-local"> from an ISO string, in local time. */
export function toLocalInput(iso: string | null): string {
	if (!iso) return '';
	const d = new Date(iso);
	const pad = (n: number) => String(n).padStart(2, '0');
	return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function fromLocalInput(v: string): string | null {
	return v ? new Date(v).toISOString() : null;
}

/** Splits a comment into text and @mention pieces for rendering. */
export function parseMentions(body: string): ({ text: string } | { name: string; id: string })[] {
	const out: ({ text: string } | { name: string; id: string })[] = [];
	const re = /@\[([^\]]+)\]\(([0-9a-f-]{36})\)/g;
	let last = 0;
	for (const m of body.matchAll(re)) {
		if (m.index! > last) out.push({ text: body.slice(last, m.index) });
		out.push({ name: m[1]!, id: m[2]! });
		last = m.index! + m[0].length;
	}
	if (last < body.length) out.push({ text: body.slice(last) });
	return out;
}

/** Colour classes for board columns (literal so Tailwind keeps them). */
export const STAGE_COLORS: Record<string, string> = {
	slate: 'bg-slate-400',
	sky: 'bg-sky-500',
	indigo: 'bg-indigo-500',
	violet: 'bg-violet-500',
	emerald: 'bg-emerald-500',
	amber: 'bg-amber-500',
	orange: 'bg-orange-500',
	rose: 'bg-rose-500',
	teal: 'bg-teal-500'
};

// ---------- Scrum ----------

export interface Sprint {
	id: string;
	name: string;
	goal: string | null;
	status: 'planned' | 'active' | 'completed';
	sort: number;
	starts_at: string | null;
	ends_at: string | null;
	committed_points: number | null;
	completed_points: number | null;
	started_at: string | null;
	completed_at: string | null;
	task_count: number;
	done_count: number;
	points_total: number;
	points_done: number;
}

export interface Epic {
	id: string;
	name: string;
	description: string | null;
	color: string;
	task_count: number;
	done_count: number;
	points_total: number;
	points_done: number;
}

export interface ScrumOverview {
	scrum_master: { id: string; full_name: string } | null;
	sprint_days: number;
	can_plan: boolean;
	can_contribute: boolean;
	sprints: Sprint[];
	completed_sprints: number;
	epics: Epic[];
	dod: { id: string; title: string }[];
	backlog: { count: number; points: number; unestimated: number };
}

export const EPIC_COLORS: Record<string, string> = {
	violet: 'bg-violet-100 text-violet-800',
	sky: 'bg-sky-100 text-sky-800',
	emerald: 'bg-emerald-100 text-emerald-800',
	amber: 'bg-amber-100 text-amber-800',
	rose: 'bg-rose-100 text-rose-800',
	indigo: 'bg-indigo-100 text-indigo-800',
	teal: 'bg-teal-100 text-teal-800',
	orange: 'bg-orange-100 text-orange-800',
	slate: 'bg-slate-200 text-slate-800'
};

/** The usual planning-poker scale. */
export const POINT_SCALE = [0, 0.5, 1, 2, 3, 5, 8, 13, 20, 40, 100];

export const fmtPoints = (p: number | null | undefined) => (p == null ? '–' : Number.isInteger(p) ? String(p) : p.toFixed(1));
