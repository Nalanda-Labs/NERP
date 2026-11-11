// Whiteboards: keeping an Excalidraw drawing in step with the server.
//
// Each change sends only the elements whose version moved since they were last sent or
// received; what comes back (and what others save) is merged element by element with the
// same rule as the server: the higher version wins, then the lower versionNonce.

export type El = { id: string; version: number; versionNonce: number; isDeleted?: boolean; [k: string]: unknown };
export type BoardFile = { key: string; file_id?: string; mime: string };
export type SceneReply = { version: number; unchanged?: boolean; scene?: { elements: El[]; appState: Record<string, unknown> }; files?: BoardFile[]; viewers?: { id: string; name: string }[]; level?: string };

export interface Transport {
	load(since: number | null): Promise<SceneReply>;
	save(body: { elements: El[]; appState?: Record<string, unknown> }): Promise<{ version: number; elements: El[] }>;
	addFile(key: string, dataURL: string): Promise<void>;
	file(key: string): Promise<Blob>;
}

/** Whether `a` replaces `b`. */
export const newer = (a: El, b: El) => a.version > b.version || (a.version === b.version && a.versionNonce < b.versionNonce);

export function mergeElements(local: readonly El[], remote: readonly El[]): El[] {
	const out = new Map(local.map((e) => [e.id, e]));
	for (const r of remote) {
		const l = out.get(r.id);
		if (!l || newer(r, l)) out.set(r.id, r);
	}
	return [...out.values()];
}

const asDataURL = (b: Blob) => new Promise<string>((res, rej) => { const fr = new FileReader(); fr.onload = () => res(fr.result as string); fr.onerror = rej; fr.readAsDataURL(b); });

/* eslint-disable @typescript-eslint/no-explicit-any */
export class BoardSync {
	version = 0;
	viewers = $state.raw<{ id: string; name: string }[]>([]);
	saving = $state(false);
	error = $state('');
	private api: any = null;
	private seen = new Map<string, number>();
	private files = new Set<string>();
	private timer: ReturnType<typeof setTimeout> | undefined;
	private pending = false;
	private bg = '';
	private capture: any;

	constructor(private t: Transport, private editable: boolean) {}

	/** Called once Excalidraw is ready, with the first scene. */
	async attach(api: any, reply: SceneReply, capture: any) {
		this.api = api;
		this.capture = capture;
		this.version = reply.version;
		for (const e of reply.scene?.elements ?? []) this.seen.set(e.id, e.version);
		this.bg = String(reply.scene?.appState?.viewBackgroundColor ?? '');
		if (reply.viewers) this.viewers = reply.viewers;
		await this.loadFiles(reply.files ?? []);
	}

	private async loadFiles(list: BoardFile[]) {
		const fresh = list.filter((f) => !this.files.has(f.key));
		for (const f of fresh) this.files.add(f.key);
		const loaded = await Promise.all(fresh.map(async (f) => {
			try {
				return { id: f.key, dataURL: await asDataURL(await this.t.file(f.key)), mimeType: f.mime, created: Date.now() };
			} catch {
				return null;
			}
		}));
		const ok = loaded.filter(Boolean);
		if (ok.length && this.api) this.api.addFiles(ok);
	}

	/** Excalidraw's onChange: send what moved (a moment later, together). */
	changed(elements: readonly El[], appState: any, files: Record<string, any>) {
		if (!this.editable || !this.api) return;
		for (const [key, f] of Object.entries(files ?? {})) {
			if (!this.files.has(key) && f?.dataURL) {
				this.files.add(key);
				this.t.addFile(key, f.dataURL).catch(() => this.files.delete(key));
			}
		}
		const moved = elements.some((e) => this.seen.get(e.id) !== e.version) || (appState?.viewBackgroundColor ?? '') !== this.bg;
		if (!moved) return;
		clearTimeout(this.timer);
		this.timer = setTimeout(() => this.flush(), 400);
	}

	async flush() {
		if (!this.api) return;
		if (this.saving) {
			this.pending = true;
			return;
		}
		const all: El[] = this.api.getSceneElementsIncludingDeleted();
		const send = all.filter((e) => this.seen.get(e.id) !== e.version);
		const st = this.api.getAppState();
		const bgChanged = (st.viewBackgroundColor ?? '') !== this.bg;
		if (!send.length && !bgChanged) return;
		this.saving = true;
		try {
			for (const e of send) this.seen.set(e.id, e.version);
			const r = await this.t.save({ elements: send, appState: bgChanged ? { viewBackgroundColor: st.viewBackgroundColor } : undefined });
			this.bg = st.viewBackgroundColor ?? '';
			this.error = '';
			this.apply(r.elements, r.version);
		} catch (e: any) {
			for (const x of send) this.seen.delete(x.id);
			this.error = e?.message ?? 'Saving failed';
		} finally {
			this.saving = false;
			if (this.pending) {
				this.pending = false;
				this.flush();
			}
		}
	}

	/** Takes in someone else's changes. */
	private apply(remote: El[], version: number, appState?: Record<string, unknown>) {
		this.version = Math.max(this.version, version);
		const local: El[] = this.api.getSceneElementsIncludingDeleted();
		const merged = mergeElements(local, remote);
		const changed = merged.some((e, i) => e !== local[i]) || merged.length !== local.length;
		for (const e of remote) this.seen.set(e.id, Math.max(this.seen.get(e.id) ?? 0, e.version));
		const bg = appState?.viewBackgroundColor as string | undefined;
		if (bg !== undefined) this.bg = bg;
		if (changed || bg !== undefined) {
			this.api.updateScene({ elements: merged, ...(bg !== undefined ? { appState: { viewBackgroundColor: bg } } : {}), captureUpdate: this.capture?.NEVER });
		}
	}

	/** Asks for what's newer than ours. */
	async pull() {
		if (!this.api) return;
		try {
			const r = await this.t.load(this.version);
			if (r.viewers) this.viewers = r.viewers;
			if (!r.unchanged && r.scene) {
				this.apply(r.scene.elements, r.version, r.scene.appState);
				await this.loadFiles(r.files ?? []);
			}
		} catch {
			/* next time */
		}
	}
}
