// Live connection to the server. Connects after sign-in, reconnects with backoff,
// and dispatches incoming events to whoever subscribed with `on()`.

import { api } from './api.ts';

type Handler = (data: any) => void;

class Realtime {
	connected = $state(false);
	private socket: WebSocket | null = null;
	private handlers = new Map<string, Set<Handler>>();
	private retry = 0;
	private timer: ReturnType<typeof setTimeout> | null = null;
	private wanted = false;

	on(type: string, handler: Handler): () => void {
		if (!this.handlers.has(type)) this.handlers.set(type, new Set());
		this.handlers.get(type)!.add(handler);
		return () => this.handlers.get(type)?.delete(handler);
	}

	start() {
		if (this.wanted) return;
		this.wanted = true;
		this.connect();
	}

	stop() {
		this.wanted = false;
		if (this.timer) clearTimeout(this.timer);
		this.socket?.close();
		this.socket = null;
		this.connected = false;
	}

	private async connect() {
		if (!this.wanted) return;
		try {
			const { ticket } = await api.post<{ ticket: string }>('/realtime/ticket');
			const proto = location.protocol === 'https:' ? 'wss' : 'ws';
			const ws = new WebSocket(`${proto}://${location.host}/api/realtime/ws?ticket=${encodeURIComponent(ticket)}`);
			this.socket = ws;
			ws.onopen = () => {
				this.connected = true;
				this.retry = 0;
			};
			ws.onmessage = (e) => {
				try {
					const msg = JSON.parse(e.data) as { type: string; data: unknown };
					this.handlers.get(msg.type)?.forEach((h) => h(msg.data));
				} catch {
					/* ignore malformed */
				}
			};
			ws.onclose = () => {
				this.connected = false;
				if (this.socket === ws) this.socket = null;
				this.schedule();
			};
		} catch {
			this.schedule();
		}
	}

	private schedule() {
		if (!this.wanted) return;
		// 1s, 2s, 4s … up to 30s, with jitter so many clients don't reconnect in lockstep.
		const delay = Math.min(30_000, 1000 * 2 ** this.retry) * (0.75 + Math.random() / 2);
		this.retry++;
		this.timer = setTimeout(() => this.connect(), delay);
	}
}

export const realtime = new Realtime();
