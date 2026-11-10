// Booking: resources, services, bookings and forms.

export interface Resource {
	id: string;
	name: string;
	kind: 'person' | 'room' | 'equipment' | 'other';
	description: string | null;
	user_id: string | null;
	user_name: string | null;
	capacity: number;
	allow_overbooking: boolean;
	color: string;
	timezone: string;
	slot_minutes: number;
	buffer_minutes: number;
	min_notice_minutes: number;
	max_days_ahead: number;
	is_active: boolean;
	sort: number;
	hours: { weekday: number; start_min: number; end_min: number }[];
	days_off: { day: string; note: string | null }[];
}

export interface Service {
	id: string;
	name: string;
	description: string | null;
	duration_minutes: number;
	price: string;
	is_extra: boolean;
	resource_ids: string[];
	is_active: boolean;
	sort: number;
}

export type BookingStatus = 'booked' | 'confirmed' | 'came' | 'no_show' | 'cancelled' | 'waitlist';

export interface Booking {
	id: string;
	number: number;
	starts_at: string;
	ends_at: string;
	status: BookingStatus;
	service_id: string | null;
	service_name: string | null;
	extra_ids: string[];
	price: string;
	client_name: string;
	client_email: string | null;
	client_phone: string | null;
	contact_id: string | null;
	deal_id: string | null;
	note: string | null;
	source: string;
	overbooked: boolean;
	created_by: string | null;
	created_by_name: string | null;
	created_at: string;
	resource_ids: string[];
}

export interface BookingForm {
	id: string;
	name: string;
	slug: string;
	intro: string | null;
	service_ids: string[];
	resource_ids: string[];
	ask_phone: boolean;
	phone_required: boolean;
	choose_resource: boolean;
	waitlist: boolean;
	thanks: string | null;
	is_active: boolean;
}

export const BOOKING_STATUS: Record<BookingStatus, { label: string; badge: string }> = {
	booked: { label: 'Booked', badge: 'bg-sky-50 text-sky-700' },
	confirmed: { label: 'Confirmed', badge: 'bg-emerald-50 text-emerald-700' },
	came: { label: 'Came', badge: 'bg-slate-200 text-slate-700' },
	no_show: { label: 'No-show', badge: 'bg-amber-50 text-amber-800' },
	cancelled: { label: 'Cancelled', badge: 'bg-slate-100 text-slate-400 line-through' },
	waitlist: { label: 'Waiting list', badge: 'bg-violet-50 text-violet-700' }
};

export const RESOURCE_KINDS: Record<Resource['kind'], string> = { person: 'Person', room: 'Room', equipment: 'Equipment', other: 'Other' };

export const TILE: Record<string, string> = {
	sky: 'bg-sky-100 border-sky-300 text-sky-900',
	indigo: 'bg-indigo-100 border-indigo-300 text-indigo-900',
	violet: 'bg-violet-100 border-violet-300 text-violet-900',
	emerald: 'bg-emerald-100 border-emerald-300 text-emerald-900',
	amber: 'bg-amber-100 border-amber-300 text-amber-900',
	rose: 'bg-rose-100 border-rose-300 text-rose-900',
	teal: 'bg-teal-100 border-teal-300 text-teal-900',
	orange: 'bg-orange-100 border-orange-300 text-orange-900',
	slate: 'bg-slate-100 border-slate-300 text-slate-900'
};

export const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
export const hhmm = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
export const toMin = (t: string) => {
	const [h, m] = t.split(':').map(Number);
	return (h ?? 0) * 60 + (m ?? 0);
};
