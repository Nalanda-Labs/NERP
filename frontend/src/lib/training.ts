// Training: types and words shared by the training pages.

export type QuestionKind = 'single' | 'multiple' | 'order' | 'text';

export interface Enrolment {
	source: 'self' | 'assigned';
	due_on: string | null;
	enrolled_at: string;
	completed_at: string | null;
}

export interface Progress {
	lessons: number;
	lessons_done: number;
	tests: number;
	tests_passed: number;
	percent: number;
}

export interface CourseCard {
	id: string;
	title: string;
	excerpt: string;
	cover_file_id: string | null;
	is_published: boolean;
	lessons: number;
	tests: number;
	enrolment: Enrolment | null;
	progress: Progress | null;
	certificate_id: string | null;
	overdue: boolean;
}

export interface LessonItem {
	id: string;
	chapter_id: string | null;
	title: string;
	sort: number;
	opens_after_days: number;
	opens_at: string | null;
	open: boolean;
	done: boolean;
}

export interface TestItem {
	id: string;
	title: string;
	description: string;
	html: string;
	is_active: boolean;
	questions: number;
	time_limit_minutes: number;
	pass_percent: number;
	attempt_limit: number;
	navigation: 'strict' | 'skip' | 'free';
	show_answers: 'never' | 'after_each' | 'at_end';
	wait_minutes: number;
	prerequisite_test_id: string | null;
	prerequisite_percent: number;
	lesson_id: string | null;
	attempts_used: number;
	attempts_left: number | null;
	best_percent: number | null;
	passed: boolean;
	in_progress: string | null;
	waiting_review: boolean;
	can_start: boolean;
	why_not: string | null;
	next_try_at: string | null;
}

export interface CourseView {
	course: { id: string; title: string; description: string; cover_file_id: string | null; is_published: boolean; to_all: boolean; sort: number; created_at: string };
	html: string;
	chapters: { id: string; title: string; sort: number }[];
	lessons: LessonItem[];
	tests: TestItem[];
	enrolment: Enrolment | null;
	can_enrol: boolean;
	progress: Progress | null;
	certificate: { id: string; number: number } | null;
	enrolled: number;
	can_manage: boolean;
	audience?: { users: { id: string; name: string; avatar_file_id?: string | null }[]; departments: string[] };
}

export interface Lesson {
	id: string;
	course: { id: string; title: string };
	chapter_id: string | null;
	title: string;
	body: string;
	html: string;
	video_url: string | null;
	opens_after_days: number;
	sort: number;
	files: { id: string; name: string; size: number; mime_type: string }[];
	done: boolean;
	enrolled: boolean;
	self_checks: { id: string; kind: QuestionKind; html: string; answers: { id: string; text: string }[] }[];
	previous: string | null;
	next: string | null;
	can_manage: boolean;
}

export interface AttemptQuestion {
	question_id: string;
	position: number;
	kind: QuestionKind;
	html: string;
	points: number;
	required_correct: boolean;
	answers: { id: string; text: string }[];
	answered: boolean;
	chosen: string[];
	written: string | null;
	correct: boolean | null;
	earned: number | null;
	right_answers: string[] | null;
	explanation: string | null;
	review_note: string | null;
}

export interface Attempt {
	id: string;
	status: 'in_progress' | 'review' | 'done';
	started_at: string;
	deadline_at: string | null;
	finished_at: string | null;
	seconds_left: number | null;
	score: number | null;
	max_score: number;
	percent: number | null;
	passed: boolean | null;
	mark: string | null;
	user_id: string;
	test: { id: string; title: string; navigation: 'strict' | 'skip' | 'free'; show_answers: string; pass_percent: number; time_limit_minutes: number };
	course: { id: string; title: string };
	questions: AttemptQuestion[];
	current: number | null;
}

export const KIND_LABEL: Record<QuestionKind, string> = {
	single: 'One answer',
	multiple: 'Several answers',
	order: 'Put in order',
	text: 'Written answer'
};

export const NAVIGATION: { value: 'strict' | 'skip' | 'free'; label: string }[] = [
	{ value: 'free', label: 'Go anywhere and change answers' },
	{ value: 'skip', label: 'May skip questions; answers are final' },
	{ value: 'strict', label: 'One by one, in turn; answers are final' }
];

export const SHOW_ANSWERS: { value: 'never' | 'after_each' | 'at_end'; label: string }[] = [
	{ value: 'at_end', label: 'At the end' },
	{ value: 'after_each', label: 'After each answer' },
	{ value: 'never', label: 'Never' }
];

export const day = (iso: string) => new Date(iso.length === 10 ? iso + 'T00:00' : iso).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
