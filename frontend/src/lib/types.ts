export interface Ref {
	id: string;
	name: string;
}

export interface RoleRef {
	id: string;
	code: string;
	name: string;
}

export interface User {
	id: string;
	email: string;
	first_name: string;
	last_name: string;
	middle_name: string | null;
	full_name: string;
	position: string | null;
	phone: string | null;
	mobile: string | null;
	birthday: string | null;
	gender: 'male' | 'female' | 'other' | null;
	about: string | null;
	avatar_file_id: string | null;
	timezone: string;
	locale: string;
	is_active: boolean;
	invite_pending: boolean;
	is_guest?: boolean;
	invited_at: string | null;
	two_factor_enabled: boolean;
	last_login_at: string | null;
	created_at: string;
	departments: Ref[];
	roles: RoleRef[];
	last_seen_at?: string | null;
	online?: boolean;
}

export interface Me extends User {
	permissions: string[];
}

export interface Session {
	access_token: string;
	expires_in: number;
	user: Me;
}

export interface Page<T> {
	items: T[];
	total: number;
	limit: number;
	offset: number;
}

export interface Department {
	id: string;
	parent_id: string | null;
	name: string;
	description: string | null;
	head_user_id: string | null;
	head_name: string | null;
	head_avatar_file_id: string | null;
	sort: number;
	member_count: number;
	created_at: string;
	deputy_user_id?: string | null;
	hidden?: boolean;
	deputy_name?: string | null;
}

export interface Role {
	id: string;
	code: string;
	name: string;
	description: string | null;
	is_system: boolean;
	user_count: number;
	permissions: string[];
}

export interface PermissionInfo {
	code: string;
	group: string;
	name: string;
	description: string;
}

export interface FileMeta {
	id: string;
	owner_id: string | null;
	original_name: string;
	mime_type: string;
	size_bytes: number;
	sha256: string;
	is_public: boolean;
	created_at: string;
}

export interface AuditEntry {
	id: string;
	actor_id: string | null;
	actor_name: string | null;
	action: string;
	entity_type: string;
	entity_id: string | null;
	details: Record<string, unknown>;
	ip: string | null;
	created_at: string;
}

// Text settings, plus switches such as require_two_factor.
export type Settings = Record<string, any>;

export const PERM = {
	USERS_VIEW: 'users.view',
	USERS_MANAGE: 'users.manage',
	DEPARTMENTS_MANAGE: 'departments.manage',
	ROLES_MANAGE: 'roles.manage',
	FILES_MANAGE: 'files.manage',
	SETTINGS_MANAGE: 'settings.manage',
	AUDIT_VIEW: 'audit.view',
	SYSTEM_MANAGE: 'system.manage',
	PRIVACY_MANAGE: 'privacy.manage',
	COMMISSIONS_MANAGE: 'commissions.manage',
	PROJECTS_CREATE: 'projects.create',
	TASKS_MANAGE_ALL: 'tasks.manage_all',
	CRM_ADMIN: 'crm.admin',
	API_KEYS: 'api.keys',
	WEBHOOKS_MANAGE: 'webhooks.manage',
	MARKETING: 'marketing.manage',
	SHOP: 'shop.manage',
	INVENTORY: 'inventory.manage',
	KB: 'kb.manage',
	HR: 'hr.manage',
	BOOKING: 'booking.manage',
	TRAINING: 'training.manage',
	SUPPORT_WORK: 'support.work',
	SUPPORT_MANAGE: 'support.manage',
	SITES: 'sites.manage',
	LISTS: 'lists.manage',
	DRIVE: 'drive.manage',
	ACCOUNTING_VIEW: 'accounting.view',
	ACCOUNTING_POST: 'accounting.post',
	ACCOUNTING_ADMIN: 'accounting.admin',
	BI: 'bi.manage',
	USERS_INVITE: 'users.invite',
	FEED_MODERATE: 'feed.moderate',
	MAIL_ADMIN: 'mail.admin'
} as const;

export interface Notification {
	id: string;
	kind: string;
	title: string;
	body: string | null;
	link: string | null;
	actor_id: string | null;
	actor_name: string | null;
	actor_avatar_file_id: string | null;
	read_at: string | null;
	created_at: string;
}

export interface SessionInfo {
	id: string;
	current: boolean;
	created_at: string;
	last_used_at: string;
	ip: string | null;
	user_agent: string | null;
}

export interface Job {
	id: string;
	kind: string;
	payload: Record<string, unknown>;
	status: 'pending' | 'running' | 'done' | 'failed';
	run_at: string;
	attempts: number;
	max_attempts: number;
	locked_by: string | null;
	last_error: string | null;
	created_at: string;
	updated_at: string;
}

/** Returned by /auth/login when the account has two-factor authentication. */
export interface TwoFactorChallenge {
	two_factor_required: true;
	challenge: string;
}
