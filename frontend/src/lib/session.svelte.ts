import { api, onSessionChange, refreshSession, setAccessToken } from './api';
import { notifications } from './notifications.svelte.ts';
import { realtime } from './realtime.svelte.ts';
import type { Me, Session } from './types';

class SessionStore {
	user = $state<Me | null>(null);
	companyName = $state<string>('NERP');
	ready = $state(false);

	constructor() {
		onSessionChange((s) => {
			this.user = s?.user ?? null;
			if (s) this.goLive();
			else this.goOffline();
		});
		// Roles or permissions changed elsewhere: reload who we are.
		realtime.on('me.changed', () => this.reloadMe());
		// This device was signed out remotely (from another device or by an admin).
		realtime.on('session.revoked', () => {
			setAccessToken(null);
			this.user = null;
			this.goOffline();
		});
	}

	private goLive() {
		realtime.start();
		notifications.start();
	}

	private goOffline() {
		realtime.stop();
		notifications.stop();
	}

	can(permission: string): boolean {
		const p = this.user?.permissions ?? [];
		return p.includes('*') || p.includes(permission);
	}

	/** Applies a session returned by login, setup, refresh or password change. */
	apply(s: Session) {
		setAccessToken(s.access_token);
		this.user = s.user;
		this.ready = true;
		this.goLive();
	}

	/** Restores the session from the refresh cookie. Returns false when signed out. */
	async restore(): Promise<boolean> {
		if (this.user) return true;
		const s = await refreshSession();
		this.ready = true;
		return !!s;
	}

	async reloadMe() {
		this.user = await api.get<Me>('/auth/me');
	}

	async logout() {
		try {
			await api.post('/auth/logout');
		} finally {
			setAccessToken(null);
			this.user = null;
			this.goOffline();
		}
	}
}

export const session = new SessionStore();
