// A website on its own domain: the reverse proxy sends that host to NERP, and every page
// address there shows the site's page of the same address (/about → /s/<site>/about). Public
// pages the site links to (the shop, web forms, booking…) keep their own addresses.
import type { Reroute } from '@sveltejs/kit/hooks';

const KEEP = [
	'/s/',
	'/shop',
	'/f/',
	'/c/',
	'/b/',
	'/d/',
	'/dash/',
	'/book',
	'/help',
	'/meet',
	'/rsvp',
	'/sign',
	'/unsubscribe',
	'/join'
];
let site: Promise<string | null> | undefined;

function siteOf(host: string): Promise<string | null> {
	site ??= fetch(`/api/public/site-domain?host=${encodeURIComponent(host)}`)
		.then((r) => (r.ok ? r.json() : null))
		.then((d) => d?.slug ?? null)
		.catch(() => null);
	return site;
}

export const reroute: Reroute = async ({ url }) => {
	// NERP itself (and development) never asks.
	if (url.hostname === 'localhost' || url.hostname === '127.0.0.1' || url.hostname === '[::1]')
		return;
	if (
		KEEP.some(
			(k) =>
				url.pathname === k ||
				url.pathname.startsWith(k.endsWith('/') ? k : `${k}/`) ||
				url.pathname === k.replace(/\/$/, '')
		)
	)
		return;
	const slug = await siteOf(url.hostname);
	if (slug) return `/s/${slug}${url.pathname === '/' ? '' : url.pathname}`;
};
