// Company-wide rules the pages adapt to (read once per page load).
import { api } from '#lib/api.ts';

let links: Promise<boolean> | null = null;

/** Whether people may make links anyone can open (files, boards, dashboards). */
export function publicLinksAllowed(): Promise<boolean> {
	links ??= api
		.get<Record<string, unknown>>('/settings')
		.then((s) => s.public_links !== false)
		.catch(() => true);
	return links;
}
