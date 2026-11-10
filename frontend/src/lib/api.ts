// Thin client for the NERP API.
// The access token lives only in memory; the refresh token is an httpOnly cookie the
// browser sends to /api/auth/refresh. A 401 triggers one silent refresh and a retry.

import type { Session } from './types';

export class ApiError extends Error {
	constructor(
		public status: number,
		public code: string,
		message: string
	) {
		super(message);
	}
}

let accessToken: string | null = null;
let refreshing: Promise<Session | null> | null = null;
let onSession: ((s: Session | null) => void) | null = null;

export function setAccessToken(token: string | null) {
	accessToken = token;
}

/** Lets the session store learn about tokens refreshed behind the scenes. */
export function onSessionChange(cb: (s: Session | null) => void) {
	onSession = cb;
}

async function parseError(res: Response): Promise<ApiError> {
	let code = 'error';
	let message = res.statusText || 'Request failed';
	try {
		const text = await res.text();
		try {
			const body = JSON.parse(text);
			code = body?.error?.code ?? code;
			message = body?.error?.message ?? message;
		} catch {
			if (text) message = text;
		}
	} catch {
		/* ignore */
	}
	return new ApiError(res.status, code, message);
}

/** Exchanges the refresh cookie for a new session. Concurrent callers share one request. */
export function refreshSession(): Promise<Session | null> {
	refreshing ??= (async () => {
		try {
			const res = await fetch('/api/auth/refresh', { method: 'POST', credentials: 'same-origin' });
			if (!res.ok) {
				accessToken = null;
				onSession?.(null);
				return null;
			}
			const session = (await res.json()) as Session;
			accessToken = session.access_token;
			onSession?.(session);
			return session;
		} finally {
			refreshing = null;
		}
	})();
	return refreshing;
}

type Body = BodyInit | object | undefined;

/** Endpoints where a 401 means "wrong credentials", not "access token expired". */
const NO_REFRESH = new Set(['/auth/login', '/auth/login/2fa', '/auth/refresh', '/auth/logout']);

export async function request<T>(method: string, path: string, body?: Body, retry = true): Promise<T> {
	const headers: Record<string, string> = {};
	if (accessToken) headers['Authorization'] = `Bearer ${accessToken}`;
	let payload: BodyInit | undefined;
	if (body instanceof FormData || body instanceof Blob) {
		payload = body;
	} else if (body !== undefined) {
		headers['Content-Type'] = 'application/json';
		payload = JSON.stringify(body);
	}

	const res = await fetch(`/api${path}`, { method, headers, body: payload, credentials: 'same-origin' });

	if (res.status === 401 && retry && !NO_REFRESH.has(path)) {
		const session = await refreshSession();
		if (session) return request<T>(method, path, body, false);
	}
	if (!res.ok) throw await parseError(res);
	if (res.status === 204) return undefined as T;
	return (await res.json()) as T;
}

export const api = {
	get: <T>(path: string) => request<T>('GET', path),
	post: <T>(path: string, body?: Body) => request<T>('POST', path, body),
	put: <T>(path: string, body?: Body) => request<T>('PUT', path, body),
	patch: <T>(path: string, body?: Body) => request<T>('PATCH', path, body),
	del: <T>(path: string) => request<T>('DELETE', path)
};

export function query(params: Record<string, string | number | boolean | null | undefined>): string {
	const q = new URLSearchParams();
	for (const [k, v] of Object.entries(params)) {
		if (v !== undefined && v !== null && v !== '') q.set(k, String(v));
	}
	const s = q.toString();
	return s ? `?${s}` : '';
}

/** Like `request`, but returns the response body as a Blob (e.g. a rendered PDF). */
export async function requestBlob(method: string, path: string, body?: object, retry = true): Promise<Blob> {
	const headers: Record<string, string> = { 'Content-Type': 'application/json' };
	if (accessToken) headers['Authorization'] = `Bearer ${accessToken}`;
	const res = await fetch(`/api${path}`, { method, headers, body: body ? JSON.stringify(body) : undefined, credentials: 'same-origin' });
	if (res.status === 401 && retry && (await refreshSession())) return requestBlob(method, path, body, false);
	if (!res.ok) throw await parseError(res);
	return res.blob();
}

/** Opens a private file (e.g. a PDF) in a new tab. */
export async function openFile(id: string, retry = true): Promise<void> {
	const win = window.open('', '_blank');
	try {
		const res = await fetch(fileUrl(id), { headers: accessToken ? { Authorization: `Bearer ${accessToken}` } : {} });
		if (res.status === 401 && retry && (await refreshSession())) {
			win?.close();
			return openFile(id, false);
		}
		if (!res.ok) throw await parseError(res);
		const href = URL.createObjectURL(await res.blob());
		if (win) win.location.href = href;
		else window.location.href = href;
		setTimeout(() => URL.revokeObjectURL(href), 60_000);
	} catch (e) {
		win?.close();
		throw e;
	}
}

/** URL for a public file (avatars). Private files must be fetched with `downloadFile`. */
export function fileUrl(id: string): string {
	return `/api/files/${id}/content`;
}

/** Downloads a private file with the bearer token and hands it to the browser. */
export async function downloadFile(id: string, name: string, retry = true): Promise<void> {
	return downloadUrl(fileUrl(id), name, retry);
}

/** File and drive downloads that can go straight to disk through a download link. */
const LINKABLE = /^\/api\/(files\/[0-9a-f-]{36}\/content|drive\/nodes\/[0-9a-f-]{36}\/download)(\?|$)/;

/** A short-lived URL for a private file (download or play it without the bearer token). */
export async function streamUrl(url: string): Promise<string> {
	return (await api.post<{ url: string }>('/download-links', { path: url })).url;
}

/** Downloads any authenticated URL (e.g. a CSV export) and saves it as `name`. Files go
 *  straight to disk through a download link, so large ones never sit in memory. */
export async function downloadUrl(url: string, name: string, retry = true): Promise<void> {
	if (LINKABLE.test(url)) {
		const a = document.createElement('a');
		// The server's file name wins over `download`, so it is told the name too.
		a.href = `${await streamUrl(url)}&name=${encodeURIComponent(name)}`;
		a.download = name;
		document.body.appendChild(a);
		a.click();
		a.remove();
		return;
	}
	const res = await fetch(url, {
		headers: accessToken ? { Authorization: `Bearer ${accessToken}` } : {}
	});
	if (res.status === 401 && retry && (await refreshSession())) return downloadUrl(url, name, false);
	if (!res.ok) throw await parseError(res);
	const href = URL.createObjectURL(await res.blob());
	const a = document.createElement('a');
	a.href = href;
	a.download = name;
	a.click();
	setTimeout(() => URL.revokeObjectURL(href), 10_000);
}

/** A request that still goes out while the page is closing (leaving a call, for one). */
export function sendOnUnload(path: string, body: object) {
	const headers: Record<string, string> = { 'Content-Type': 'application/json' };
	if (accessToken) headers['Authorization'] = `Bearer ${accessToken}`;
	fetch(`/api${path}`, { method: 'POST', headers, body: JSON.stringify(body), keepalive: true, credentials: 'same-origin' }).catch(() => {});
}
