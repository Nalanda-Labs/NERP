// Live "my work" state shown in the app chrome: task counters and the running timer.

import { api } from './api.ts';
import { realtime } from './realtime.svelte.ts';
import type { TaskCounters } from './tasks.ts';

type Timer = { task_id: string; number: number; title: string; started_at: string } | null;

class Work {
	counters = $state<TaskCounters | null>(null);
	timer = $state<Timer>(null);
	private started = false;
	private pending: ReturnType<typeof setTimeout> | undefined;

	start() {
		if (this.started) return;
		this.started = true;
		realtime.on('task.changed', () => this.refreshSoon());
		realtime.on('timer.changed', () => this.refreshSoon());
		this.refresh();
	}

	refreshSoon() {
		clearTimeout(this.pending);
		this.pending = setTimeout(() => this.refresh(), 400);
	}

	async refresh() {
		try {
			const [c, t] = await Promise.all([api.get<TaskCounters>('/tasks/counters'), api.get<Timer>('/tasks/timer')]);
			this.counters = c;
			this.timer = t;
		} catch {
			/* signed out or offline; try again on the next event */
		}
	}

	async stopTimer() {
		if (!this.timer) return;
		await api.post(`/tasks/${this.timer.task_id}/timer/stop`);
		this.timer = null;
	}
}

export const work = new Work();
