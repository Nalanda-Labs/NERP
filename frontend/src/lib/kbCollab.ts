// Live co-editing of a knowledge base page: connects a Yjs document (and the editors'
// cursors) to the server, which keeps the document and passes changes on to the page's other
// editors through the real-time connection.
import * as Y from 'yjs';
import { Awareness, applyAwarenessUpdate, encodeAwarenessUpdate, removeAwarenessStates } from 'y-protocols/awareness';
import { api } from './api.ts';
import { realtime } from './realtime.svelte.ts';

const REMOTE = 'remote';
const b64 = (u: Uint8Array) => {
	let s = '';
	for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000));
	return btoa(s);
};
const unb64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

export type Person = { client: string; name: string; color: string };

export class KbCollab {
	readonly doc = new Y.Doc();
	readonly awareness = new Awareness(this.doc);
	private client = String(this.doc.clientID);
	private off: (() => void)[] = [];
	private pending: Uint8Array[] = [];
	private flushing: ReturnType<typeof setTimeout> | null = null;
	private aware: ReturnType<typeof setTimeout> | null = null;
	private beat: ReturnType<typeof setInterval> | null = null;
	private closed = false;

	constructor(
		private pageId: string,
		me: { name: string; color: string },
		private on: { people?: (p: Person[]) => void; reset?: () => void; error?: (e: unknown) => void } = {}
	) {
		this.awareness.setLocalStateField('user', me);
	}

	/** Joins the page's live document. Returns true when this editor must put the saved text in. */
	async start(): Promise<boolean> {
		this.off.push(
			realtime.on('kb.collab', (d: { page_id: string; client: string; update: string }) => {
				if (d.page_id === this.pageId && d.client !== this.client) Y.applyUpdate(this.doc, unb64(d.update), REMOTE);
			}),
			realtime.on('kb.collab.awareness', (d: { page_id: string; client: string; update: string }) => {
				if (d.page_id === this.pageId && d.client !== this.client) applyAwarenessUpdate(this.awareness, unb64(d.update), REMOTE);
			}),
			realtime.on('kb.collab.reset', (d: { page_id: string }) => {
				if (d.page_id === this.pageId) this.on.reset?.();
			})
		);
		const r = await api.post<{ state: string | null; seed: boolean }>(`/kb/pages/${this.pageId}/collab`, { client: this.client });
		if (r.state) Y.applyUpdate(this.doc, unb64(r.state), REMOTE);
		this.doc.on('update', this.onUpdate);
		this.awareness.on('update', this.onAwareness);
		this.awareness.on('change', this.onPeople);
		this.sendAwareness();
		// A sign of life (and anything missed while the connection was down) now and then.
		this.beat = setInterval(() => this.sync(), 20_000);
		return r.seed;
	}

	/** Pulls the merged document again (missed updates apply; known ones are no-ops). */
	async sync() {
		if (this.closed) return;
		try {
			const r = await api.post<{ state: string | null }>(`/kb/pages/${this.pageId}/collab`, { client: this.client });
			if (r.state) Y.applyUpdate(this.doc, unb64(r.state), REMOTE);
			this.sendAwareness();
		} catch (e) {
			this.on.error?.(e);
		}
	}

	private onUpdate = (update: Uint8Array, origin: unknown) => {
		if (origin === REMOTE || this.closed) return;
		this.pending.push(update);
		if (!this.flushing) this.flushing = setTimeout(() => this.flush(), 60);
	};

	private async flush() {
		this.flushing = null;
		if (!this.pending.length) return;
		const update = this.pending.length === 1 ? this.pending[0]! : Y.mergeUpdates(this.pending);
		this.pending = [];
		try {
			await api.post(`/kb/pages/${this.pageId}/collab/update`, { client: this.client, update: b64(update) });
		} catch (e) {
			// Kept for the next try; the document itself still has it.
			this.pending.unshift(update);
			this.on.error?.(e);
			if (!this.flushing) this.flushing = setTimeout(() => this.flush(), 2000);
		}
	}

	private onAwareness = (_: unknown, origin: unknown) => {
		if (origin === REMOTE) return;
		if (!this.aware) this.aware = setTimeout(() => this.sendAwareness(), 120);
	};

	private sendAwareness() {
		this.aware = null;
		if (this.closed) return;
		const u = encodeAwarenessUpdate(this.awareness, [this.doc.clientID]);
		api.post(`/kb/pages/${this.pageId}/collab/awareness`, { client: this.client, update: b64(u) }).catch(() => {});
	}

	private onPeople = () => {
		const people: Person[] = [];
		this.awareness.getStates().forEach((s, id) => {
			if (id !== this.doc.clientID && s.user) people.push({ client: String(id), name: s.user.name, color: s.user.color });
		});
		this.on.people?.(people);
	};

	async destroy() {
		if (this.closed) return;
		await this.flush().catch(() => {});
		this.closed = true;
		this.off.forEach((f) => f());
		if (this.beat) clearInterval(this.beat);
		// Tell the others this editor's cursor is gone.
		removeAwarenessStates(this.awareness, [this.doc.clientID], 'local');
		const u = encodeAwarenessUpdate(this.awareness, [this.doc.clientID]);
		await api.post(`/kb/pages/${this.pageId}/collab/awareness`, { client: this.client, update: b64(u) }).catch(() => {});
		await api.post(`/kb/pages/${this.pageId}/collab/leave`, { client: this.client }).catch(() => {});
		this.awareness.destroy();
		this.doc.destroy();
	}
}
