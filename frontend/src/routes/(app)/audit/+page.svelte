<script lang="ts">
	import { onMount } from 'svelte';
	import { api, query } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { AuditEntry, Page } from '#lib/types.ts';

	const PAGE_SIZE = 50;
	let data = $state<Page<AuditEntry> | null>(null);
	let offset = $state(0);
	let action = $state('');

	const filters = [
		{ value: '', label: 'All activity' },
		{ value: 'auth.', label: 'Sign-ins' },
		{ value: 'user.', label: 'Employees' },
		{ value: 'department.', label: 'Structure' },
		{ value: 'role.', label: 'Roles' },
		{ value: 'project.', label: 'Projects' },
		{ value: 'file.', label: 'Files' },
		{ value: 'settings.', label: 'Settings' }
	];

	const labels: Record<string, string> = {
		'setup.complete': 'Set up the company',
		'auth.login': 'Signed in',
		'auth.login_failed': 'Failed sign-in',
		'auth.login_blocked': 'Sign-in blocked (deactivated)',
		'auth.password_change': 'Changed their password',
		'user.create': 'Added an employee',
		'user.update': 'Edited an employee',
		'user.deactivate': 'Deactivated an employee',
		'user.activate': 'Reactivated an employee',
		'user.roles': 'Changed roles',
		'user.password_reset': 'Reset a password',
		'department.create': 'Created a department',
		'department.update': 'Edited a department',
		'department.delete': 'Deleted a department',
		'role.create': 'Created a role',
		'role.permissions': 'Changed role permissions',
		'role.delete': 'Deleted a role',
		'file.upload': 'Uploaded a file',
		'file.delete': 'Deleted a file',
		'settings.update': 'Changed settings',
		'auth.2fa_failed': 'Wrong two-factor code',
		'auth.2fa_enabled': 'Turned on two-factor',
		'auth.2fa_disabled': 'Turned off two-factor',
		'auth.recovery_codes_regenerated': 'New recovery codes',
		'auth.password_reset_requested': 'Requested a password reset',
		'auth.password_reset': 'Reset their password by email',
		'auth.invite_accepted': 'Accepted an invitation',
		'auth.session_revoked': 'Signed out a device',
		'auth.sessions_revoked': 'Signed out other devices',
		'user.invite': 'Invited an employee',
		'user.2fa_reset': 'Turned off two-factor for an employee',
		'user.sessions_revoked': 'Signed an employee out everywhere',
		'project.create': 'Created a project',
		'project.update': 'Edited a project',
		'project.delete': 'Deleted a project'
	};

	const tone = (a: string) =>
		a.endsWith('failed') || a.endsWith('blocked') || a.endsWith('delete') || a.endsWith('deactivate') || a.endsWith('2fa_reset')
			? 'bg-red-50 text-red-700'
			: a.startsWith('auth.')
				? 'bg-sky-50 text-sky-700'
				: 'bg-slate-100 text-slate-700';

	function summary(e: AuditEntry): string {
		const d = e.details as Record<string, unknown>;
		const parts = ['email', 'name', 'code', 'company'].filter((k) => typeof d[k] === 'string').map((k) => String(d[k]));
		return parts.join(' · ');
	}

	async function load() {
		try {
			data = await api.get<Page<AuditEntry>>('/audit' + query({ action, limit: PAGE_SIZE, offset }));
		} catch (e) {
			toasts.error(e);
		}
	}

	onMount(load);
</script>

<svelte:head><title>Audit log · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Audit log</h1>
			<p class="text-sm text-slate-500">Security-relevant actions across the company.</p>
		</div>
		<select class="input w-48" bind:value={action} onchange={() => ((offset = 0), load())}>
			{#each filters as f (f.value)}<option value={f.value}>{f.label}</option>{/each}
		</select>
	</div>

	<div class="card overflow-hidden">
		<table class="min-w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs font-medium text-slate-500">
				<tr>
					<th class="px-5 py-3">When</th>
					<th class="px-5 py-3">Who</th>
					<th class="px-5 py-3">What</th>
					<th class="hidden px-5 py-3 md:table-cell">Details</th>
					<th class="hidden px-5 py-3 lg:table-cell">IP</th>
				</tr>
			</thead>
			<tbody class="divide-y divide-slate-100">
				{#each data?.items ?? [] as e (e.id)}
					<tr>
						<td class="px-5 py-3 whitespace-nowrap text-slate-500">{formatDateTime(e.created_at)}</td>
						<td class="px-5 py-3">
							{#if e.actor_id}<a class="font-medium text-slate-800 hover:underline" href="/employees/{e.actor_id}">{e.actor_name}</a>{:else}<span class="text-slate-400">—</span>{/if}
						</td>
						<td class="px-5 py-3"><span class="badge {tone(e.action)}">{labels[e.action] ?? e.action}</span></td>
						<td class="hidden max-w-xs truncate px-5 py-3 text-slate-600 md:table-cell">
							{#if e.entity_type === 'user' && e.entity_id && e.entity_id !== e.actor_id}
								<a class="text-brand-600 hover:underline" href="/employees/{e.entity_id}">View employee</a>
							{/if}
							{summary(e)}
						</td>
						<td class="hidden px-5 py-3 font-mono text-xs text-slate-500 lg:table-cell">{e.ip ?? ''}</td>
					</tr>
				{:else}
					<tr><td colspan="5" class="px-5 py-12 text-center text-slate-500">{data ? 'Nothing recorded yet.' : 'Loading…'}</td></tr>
				{/each}
			</tbody>
		</table>
		{#if data && data.total > PAGE_SIZE}
			<div class="flex items-center justify-between border-t border-slate-200 px-5 py-3 text-sm text-slate-500">
				<span>{offset + 1}–{Math.min(offset + PAGE_SIZE, data.total)} of {data.total}</span>
				<div class="flex gap-2">
					<button class="btn btn-secondary" disabled={offset === 0} onclick={() => ((offset -= PAGE_SIZE), load())}>Newer</button>
					<button class="btn btn-secondary" disabled={offset + PAGE_SIZE >= data.total} onclick={() => ((offset += PAGE_SIZE), load())}>Older</button>
				</div>
			</div>
		{/if}
	</div>
</div>
