// Guided tours: short walks through the app that point at its parts one at a time. Each is
// offered once (the first visit to where it starts); finished or skipped ones are remembered
// on the server, and "Take the tour" plays them again.
import { api } from '#lib/api.ts';

export type TourStep = { target: string; title: string; text: string };
export type Tour = { name: string; when: (path: string) => boolean; steps: TourStep[] };

export const TOURS: Tour[] = [
	{
		name: 'welcome',
		when: () => true,
		steps: [
			{ target: 'a[href="/feed"]', title: 'News feed', text: 'Company news, polls, appreciation badges, and requests (leave, expenses…) that colleagues approve right there.' },
			{ target: 'a[href="/tasks"]', title: 'Tasks', text: 'Your tasks and those you set for others, with deadlines, checklists, boards and reports.' },
			{ target: 'a[href="/messenger"]', title: 'Messenger', text: 'Chats with colleagues and groups, and calls with video and screen sharing.' },
			{ target: 'a[href="/calendar"]', title: 'Calendar', text: 'Your events and meetings; share your calendar or let clients book a slot.' },
			{ target: 'a[href="/drive"]', title: 'Drive', text: 'Files for yourself, your team and the company; open office documents to edit them together.' },
			{ target: '[data-tour="search"]', title: 'Search everything', text: 'Find people, tasks, records, files and pages from one box.' },
			{ target: '[data-tour="notifications"]', title: 'Notifications', text: 'What needs you: mentions, approvals, tasks and reminders.' },
			{ target: '[data-tour="customise"]', title: 'Make it yours', text: 'Hide the parts you don’t use, pin the ones you do, and pick a background.' },
			{ target: '[data-tour="me"]', title: 'You', text: 'Your profile, sign-in security, and this tour again whenever you want it.' }
		]
	},
	{
		name: 'crm',
		when: (p) => /^\/crm\/(lead|deal|contact|company)$/.test(p),
		steps: [
			{ target: '[data-tour="crm-new"]', title: 'Add a record', text: 'A new lead, deal, contact or company. Its stage moves on as the work does.' },
			{ target: '[data-tour="crm-filters"]', title: 'Filters', text: 'Narrow the list by stage, responsible person, dates, amounts or your own fields, and save filters you use.' },
			{ target: '[data-tour="crm-export"]', title: 'Export', text: 'Take the list away as Excel, CSV, or vCards for Outlook, Gmail and phones.' }
		]
	},
	{
		name: 'tasks',
		when: (p) => p === '/tasks',
		steps: [
			{ target: '[data-tour="task-new"]', title: 'New task', text: 'Give it to yourself or a colleague, with a deadline, observers and a checklist.' },
			{ target: '[data-tour="task-flows"]', title: 'Flows', text: 'Queues of incoming work that a team takes in turn.' },
			{ target: '[data-tour="task-reports"]', title: 'Reports', text: 'Who did what, on time or late, and where the time went.' }
		]
	}
];

class Tours {
	done = $state<string[] | null>(null);
	/** The tour playing, and its step. */
	current = $state<{ tour: Tour; step: number } | null>(null);

	async load() {
		this.done = (await api.get<{ done: string[] }>('/me/tours').catch(() => ({ done: [] as string[] }))).done;
	}
	/** Starts the first tour offered here that hasn't been seen. */
	offer(path: string) {
		if (!this.done || this.current) return;
		// Browser automation (tests) gets no tours unless it asks for them.
		let wanted = false;
		try {
			wanted = localStorage.getItem('nerp:tours-in-automation') === '1';
		} catch {
			/* storage unavailable */
		}
		if (navigator.webdriver && !wanted) return;
		const t = TOURS.find((x) => !this.done!.includes(x.name) && x.when(path));
		if (t) this.current = { tour: t, step: 0 };
	}
	async finish() {
		const name = this.current?.tour.name;
		this.current = null;
		if (name) this.done = (await api.post<{ done: string[] }>(`/me/tours/${name}`).catch(() => ({ done: [...(this.done ?? []), name] }))).done;
	}
	async replay() {
		this.done = (await api.del<{ done: string[] }>('/me/tours').catch(() => ({ done: [] as string[] }))).done;
		this.current = { tour: TOURS[0], step: 0 };
	}
}

export const tours = new Tours();
