// Flows: queues of similar tasks a team works through.

export interface Flow {
	id: string;
	project_id: string;
	project_name: string;
	name: string;
	description: string | null;
	owner_id: string;
	owner_name: string;
	distribution: 'manual' | 'queue' | 'self';
	team: string[];
	planned_hours: number | null;
	is_active: boolean;
	can_manage: boolean;
	in_team: boolean;
	stats: { waiting: number; at_work: number; done: number; avg_wait_hours: number | null; avg_completion_hours: number | null; efficiency: number | null };
	members?: { id: string; full_name: string; avatar_file_id: string | null; open: number }[];
}

export const DISTRIBUTION: Record<Flow['distribution'], { label: string; hint: string }> = {
	queue: { label: 'In turn', hint: 'each task goes to the team member with the fewest open ones' },
	manual: { label: 'By the administrator', hint: 'tasks wait for the administrator to give them out' },
	self: { label: 'Taken by the team', hint: 'tasks wait until someone in the team takes them' }
};
