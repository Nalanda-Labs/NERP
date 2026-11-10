import { api } from './api.ts';
import { realtime } from './realtime.svelte.ts';
import { toasts } from './toast.svelte.ts';
import type { Notification, Page } from './types.ts';

class Notifications {
	unread = $state(0);
	recent = $state<Notification[]>([]);
	private started = false;

	async start() {
		if (this.started) return;
		this.started = true;
		realtime.on('notification', (n: Notification) => {
			this.recent = [n, ...this.recent.filter((x) => x.id !== n.id)].slice(0, 20);
			this.unread += 1;
			toasts.info(n.title);
		});
		realtime.on('notifications.read', (d: { ids: string[] | null; unread: number }) => {
			this.unread = d.unread;
			const now = new Date().toISOString();
			this.recent = this.recent.map((n) => (!d.ids || d.ids.includes(n.id) ? { ...n, read_at: n.read_at ?? now } : n));
		});
		await this.refresh();
	}

	stop() {
		this.started = false;
		this.unread = 0;
		this.recent = [];
	}

	async refresh() {
		const [count, page] = await Promise.all([
			api.get<{ count: number }>('/notifications/unread-count'),
			api.get<Page<Notification>>('/notifications?limit=20')
		]);
		this.unread = count.count;
		this.recent = page.items;
	}

	async markRead(id: string) {
		const n = this.recent.find((x) => x.id === id);
		if (n?.read_at) return;
		const res = await api.post<{ unread: number }>(`/notifications/${id}/read`);
		this.unread = res.unread;
		this.recent = this.recent.map((x) => (x.id === id ? { ...x, read_at: new Date().toISOString() } : x));
	}

	async markAllRead() {
		const res = await api.post<{ unread: number }>('/notifications/read-all');
		this.unread = res.unread;
		const now = new Date().toISOString();
		this.recent = this.recent.map((x) => ({ ...x, read_at: x.read_at ?? now }));
	}
}

export const notifications = new Notifications();
