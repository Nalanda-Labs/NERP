// Shapes and constants of the messenger.

export type ChatKind = 'direct' | 'group' | 'channel';

export interface Chat {
	id: string;
	kind: ChatKind;
	title: string;
	description: string | null;
	other_id: string | null;
	avatar_file_id: string | null;
	members: number;
	unread: number;
	last_body: string | null;
	last_author: string | null;
	last_message_at: string;
	created_by: string | null;
	project_id: string | null;
	task_id: string | null;
	event_id: string | null;
	visibility: 'private' | 'open';
	audience: 'manual' | 'everyone' | 'department';
	department_id: string | null;
	department_name: string | null;
	is_general: boolean;
	post_policy: 'all' | 'managers';
	auto_delete_hours: number | null;
	my_role: 'owner' | 'admin' | 'member' | null;
	muted: boolean;
	folder_ids: string[];
}

export interface Person {
	id: string;
	name: string;
	avatar_file_id: string | null;
	last_read_at?: string;
	role?: 'owner' | 'admin' | 'member';
}

export interface ChatDetail extends Chat {
	people: Person[];
	can: { manage: boolean; post: boolean; join: boolean; edit_people: boolean; add_people: boolean; leave: boolean };
}

export interface MsgFile {
	id: string;
	name: string;
	size: number;
	mime: string;
}

export interface Poll {
	question: string;
	multiple: boolean;
	anonymous: boolean;
	closed: boolean;
	options: { text: string; votes: number; voters: string[] }[];
	voters: number;
	my_votes: number[];
}

export interface Msg {
	id: string;
	chat_id: string;
	user_id: string | null;
	user_name: string | null;
	avatar_file_id: string | null;
	body: string;
	kind: 'text' | 'poll' | 'voice' | 'video' | 'sticker';
	parent_id: string | null;
	files: MsgFile[];
	edited_at: string | null;
	deleted: boolean;
	created_at: string;
	comment_count: number;
	last_comment_at: string | null;
	reactions: { emoji: string; count: number; mine: boolean; names: string[] }[];
	poll: Poll | null;
}

export interface Folder {
	id: string;
	name: string;
	chat_ids: string[];
	unread: number;
}

export const REACTIONS = ['👍', '❤️', '😂', '🎉', '😮', '😢', '👏', '🙏'];

/** Stickers NERP ships with: code → what is drawn. */
export const STICKERS: Record<string, { emoji: string; label: string }> = {
	'thumbs-up': { emoji: '👍', label: 'Thumbs up' },
	party: { emoji: '🥳', label: 'Party' },
	heart: { emoji: '💖', label: 'Heart' },
	laugh: { emoji: '🤣', label: 'Laugh' },
	clap: { emoji: '👏', label: 'Applause' },
	fire: { emoji: '🔥', label: 'Fire' },
	ok: { emoji: '👌', label: 'OK' },
	thinking: { emoji: '🤔', label: 'Thinking' },
	coffee: { emoji: '☕', label: 'Coffee' },
	rocket: { emoji: '🚀', label: 'Rocket' },
	sad: { emoji: '😿', label: 'Sad' },
	wave: { emoji: '👋', label: 'Hello' }
};

export const AUTO_DELETE: [number | null, string][] = [
	[null, 'Never'],
	[1, 'After 1 hour'],
	[24, 'After 1 day'],
	[24 * 7, 'After 1 week'],
	[24 * 30, 'After 1 month'],
	[24 * 365, 'After 1 year']
];
