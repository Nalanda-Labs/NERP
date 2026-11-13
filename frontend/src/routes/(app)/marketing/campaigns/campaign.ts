// Shapes and labels shared by the campaign pages.

export interface Counts {
	recipients: number;
	sent: number;
	failed: number;
	pending: number;
	opened: number;
	clicked: number;
	unsubscribed: number;
}

export interface Campaign {
	id: string;
	name: string;
	slug: string;
	status: 'draft' | 'scheduled' | 'sending' | 'sent' | 'cancelled';
	/** How it goes out. */
	channel: 'email' | 'sms';
	segment_ids: string[];
	subject: string;
	body: string;
	/** Designed in blocks (the email designer); then body is its plain text. */
	design: import('#lib/components/marketing/EmailDesigner.svelte').Design | null;
	from_name: string | null;
	reply_to: string | null;
	send_at: string | null;
	started_at: string | null;
	finished_at: string | null;
	skipped: number;
	counts: Counts;
}

export const CAMPAIGN_STATUS: Record<string, [string, string]> = {
	draft: ['Draft', 'bg-slate-100 text-slate-700'],
	scheduled: ['Scheduled', 'bg-sky-50 text-sky-800'],
	sending: ['Sending', 'bg-amber-50 text-amber-800'],
	sent: ['Sent', 'bg-emerald-50 text-emerald-800'],
	cancelled: ['Cancelled', 'bg-slate-100 text-slate-500']
};

/** "42%" of a whole (one decimal under 10%). */
export function rate(part: number, whole: number): string {
	if (!whole) return '—';
	const p = (part / whole) * 100;
	return `${p < 10 && p > 0 ? p.toFixed(1) : Math.round(p)}%`;
}
