export function formatDate(iso: string | null | undefined): string {
	if (!iso) return '—';
	return new Date(iso).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

export function formatDateTime(iso: string | null | undefined): string {
	if (!iso) return '—';
	return new Date(iso).toLocaleString(undefined, {
		year: 'numeric',
		month: 'short',
		day: 'numeric',
		hour: '2-digit',
		minute: '2-digit'
	});
}

export function formatBytes(n: number): string {
	if (n < 1024) return `${n} B`;
	const units = ['KB', 'MB', 'GB', 'TB'];
	let v = n / 1024;
	let i = 0;
	while (v >= 1024 && i < units.length - 1) {
		v /= 1024;
		i++;
	}
	return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

export function initials(name: string): string {
	return name
		.split(/\s+/)
		.filter(Boolean)
		.slice(0, 2)
		.map((p) => p[0]!.toUpperCase())
		.join('');
}

export function formatRelative(iso: string | null | undefined): string {
	if (!iso) return '—';
	const diff = (Date.now() - new Date(iso).getTime()) / 1000;
	if (diff < 45) return 'just now';
	if (diff < 3600) return `${Math.round(diff / 60)} min ago`;
	if (diff < 86400) return `${Math.round(diff / 3600)} h ago`;
	if (diff < 7 * 86400) return `${Math.round(diff / 86400)} d ago`;
	return formatDate(iso);
}

/** A short "Firefox on Linux" style description of a user-agent string. */
export function describeDevice(ua: string | null | undefined): string {
	if (!ua) return 'Unknown device';
	const browser =
		/Edg\//.test(ua) ? 'Edge'
		: /OPR\//.test(ua) ? 'Opera'
		: /Firefox\//.test(ua) ? 'Firefox'
		: /Chrome\//.test(ua) ? 'Chrome'
		: /Safari\//.test(ua) ? 'Safari'
		: /curl|node|python|Go-http/i.test(ua) ? 'API client'
		: 'Browser';
	const os =
		/Windows/.test(ua) ? 'Windows'
		: /iPhone|iPad/.test(ua) ? 'iOS'
		: /Mac OS X|Macintosh/.test(ua) ? 'macOS'
		: /Android/.test(ua) ? 'Android'
		: /Linux/.test(ua) ? 'Linux'
		: '';
	return os ? `${browser} on ${os}` : browser;
}
