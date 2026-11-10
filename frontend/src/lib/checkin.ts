// Check-in: types and words shared by the top-bar button, its dialog and the HR → Check-in page.

export type Location = 'office' | 'home' | 'remote' | 'field' | 'custom';
export type Reason = 'illness' | 'sick_leave' | 'time_off' | 'vacation' | 'custom';
export type CheckInStatus = 'working' | 'not_working' | 'cancelled';

export interface CheckIn {
	id: string;
	user_id: string;
	day: string;
	status: CheckInStatus;
	location: Location | null;
	location_text: string | null;
	message: string | null;
	chat_id: string | null;
	chat_title: string | null;
	latitude: number | null;
	longitude: number | null;
	address: string | null;
	reason: Reason | null;
	reason_text: string | null;
	cancelled_at: string | null;
	created_at: string;
}

export interface CheckInPrefs {
	location: Location;
	location_text: string | null;
	message: string | null;
	send_message: boolean;
	send_geo: boolean;
	chat_id: string | null;
	start_workday: boolean;
	department_id: string | null;
	muted_forever: boolean;
	muted_until: string | null;
}

export interface CheckInToday {
	enabled: boolean;
	geo_enabled: boolean;
	can_manage: boolean;
	day: string;
	checkin: CheckIn | null;
	prefs: CheckInPrefs;
	remind: boolean;
	muted: boolean;
	working_day: boolean;
	workday_started?: boolean;
}

export const LOCATIONS: { value: Location; label: string; full: string; icon: string }[] = [
	{ value: 'office', label: 'Office', full: 'Working in office', icon: '🏢' },
	{ value: 'home', label: 'From home', full: 'Working from home', icon: '🏠' },
	{ value: 'remote', label: 'Remotely', full: 'Working remotely', icon: '🌐' },
	{ value: 'field', label: 'Fieldwork', full: 'Fieldwork', icon: '🚗' },
	{ value: 'custom', label: 'Other', full: 'Working', icon: '📌' }
];

export const REASONS: { value: Reason; label: string }[] = [
	{ value: 'illness', label: "I'm not feeling well" },
	{ value: 'sick_leave', label: 'Sick leave' },
	{ value: 'time_off', label: 'Leave of absence' },
	{ value: 'vacation', label: 'Vacation' },
	{ value: 'custom', label: 'Other' }
];

export const locationOf = (v: Location | null) => LOCATIONS.find((l) => l.value === v) ?? LOCATIONS[0];

/** What a check-in says, in a few words: "Working in office", "Not working: Sick leave". */
export function summary(k: CheckIn): string {
	if (k.status === 'working') return k.location === 'custom' ? (k.location_text ?? 'Working') : locationOf(k.location).full;
	return `${k.status === 'cancelled' ? 'Not working after all' : 'Not working'}: ${k.reason_text ?? ''}`;
}

export const ABSENCE: Record<string, string> = { vacation: 'Vacation', sick: 'Sick leave', personal: 'Personal day', business_trip: 'Business trip', other: 'Away' };

export const mapLink = (lat: number, lng: number) => `https://www.openstreetmap.org/?mlat=${lat.toFixed(6)}&mlon=${lng.toFixed(6)}#map=17/${lat.toFixed(6)}/${lng.toFixed(6)}`;
