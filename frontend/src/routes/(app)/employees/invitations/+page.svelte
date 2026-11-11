<script lang="ts">
	// Inviting people: by email one at a time, or with a link to share (into a department,
	// only for some email domains, a number of times, until a date, with or without approval);
	// and the requests waiting for approval.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDate, formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Link = {
		id: string;
		note: string | null;
		department: string | null;
		email_domains: string[];
		needs_approval: boolean;
		max_uses: number | null;
		uses: number;
		expires_at: string | null;
		disabled: boolean;
		created_by: string | null;
		created_at: string;
		usable: boolean;
	};
	type Request = {
		id: string;
		name: string;
		email: string;
		created_at: string;
		link_note: string | null;
		department: string | null;
	};
	let links = $state<Link[]>([]);
	let requests = $state<Request[]>([]);
	let departments = $state<{ id: string; name: string }[]>([]);
	let invite = $state({ email: '', first_name: '', last_name: '' });
	let form = $state({
		note: '',
		department_id: '',
		domains: '',
		needs_approval: true,
		max_uses: '',
		expires: ''
	});
	let madeUrl = $state('');
	let busy = $state(false);

	async function load() {
		try {
			[links, requests] = await Promise.all([
				api.get<{ items: Link[] }>('/join-links').then((r) => r.items),
				api.get<{ items: Request[] }>('/join-requests').then((r) => r.items)
			]);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(async () => {
		load();
		departments = await api.get<{ id: string; name: string }[]>('/departments').catch(() => []);
	});

	async function sendInvite(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			await api.post('/users', { ...invite, send_invite: true });
			toasts.success(`Invitation sent to ${invite.email}`);
			invite = { email: '', first_name: '', last_name: '' };
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function makeLink(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const r = await api.post<Link & { url: string }>('/join-links', {
				note: form.note || null,
				department_id: form.department_id || null,
				email_domains: form.domains.split(/[\s,]+/).filter(Boolean),
				needs_approval: form.needs_approval,
				max_uses: form.max_uses ? Number(form.max_uses) : null,
				expires_at: form.expires ? new Date(`${form.expires}T23:59:59`).toISOString() : null
			});
			madeUrl = r.url;
			form = {
				note: '',
				department_id: '',
				domains: '',
				needs_approval: true,
				max_uses: '',
				expires: ''
			};
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function toggle(l: Link) {
		await api.patch(`/join-links/${l.id}`, { disabled: !l.disabled }).catch(toasts.error);
		await load();
	}
	async function drop(l: Link) {
		if (!confirm('Delete this link? People who already joined stay.')) return;
		await api.del(`/join-links/${l.id}`).catch(toasts.error);
		await load();
	}
	async function decide(r: Request, ok: boolean) {
		if (!ok && !confirm(`Turn down ${r.name}'s request?`)) return;
		try {
			requests = (
				await api.post<{ items: Request[] }>(`/join-requests/${r.id}/${ok ? 'approve' : 'reject'}`)
			).items;
			toasts.success(ok ? `${r.name} is invited by email` : 'Request turned down');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function copy(text: string) {
		await navigator.clipboard?.writeText(text).catch(() => {});
		toasts.success('Copied');
	}
</script>

<svelte:head><title>Invitations · NERP</title></svelte:head>

<div class="max-w-4xl space-y-5">
	<div class="flex items-center gap-2">
		<a class="btn btn-ghost px-2" href="/employees" aria-label="Employees">‹</a>
		<h1 class="text-2xl font-semibold text-slate-900">Invitations</h1>
	</div>

	{#if requests.length}
		<section class="card" aria-label="Waiting for approval">
			<h2 class="border-b border-slate-100 px-5 py-3 font-semibold text-slate-900">
				Asking to join <span class="badge ml-1 bg-amber-50 text-amber-800">{requests.length}</span>
			</h2>
			<ul class="divide-y divide-slate-50 text-sm">
				{#each requests as r (r.id)}
					<li class="flex flex-wrap items-center gap-3 px-5 py-3" data-request={r.email}>
						<div class="min-w-0 flex-1">
							<p class="font-medium text-slate-900">
								{r.name} <span class="font-normal text-slate-500">{r.email}</span>
							</p>
							<p class="text-xs text-slate-500">
								{formatRelative(r.created_at)}{r.link_note
									? ` · through “${r.link_note}”`
									: ''}{r.department ? ` · ${r.department}` : ''}
							</p>
						</div>
						<button class="btn btn-primary" onclick={() => decide(r, true)}>Approve</button>
						<button class="btn btn-ghost hover:text-red-600" onclick={() => decide(r, false)}
							>Turn down</button
						>
					</li>
				{/each}
			</ul>
		</section>
	{/if}

	<section class="card p-5" aria-label="Invite by email">
		<h2 class="mb-3 font-semibold text-slate-900">Invite by email</h2>
		<form class="grid gap-3 text-sm sm:grid-cols-4" onsubmit={sendInvite}>
			<input
				class="input"
				type="email"
				required
				placeholder="Work email"
				aria-label="Email"
				bind:value={invite.email}
			/>
			<input
				class="input"
				required
				maxlength="100"
				placeholder="First name"
				aria-label="First name"
				bind:value={invite.first_name}
			/>
			<input
				class="input"
				required
				maxlength="100"
				placeholder="Last name"
				aria-label="Last name"
				bind:value={invite.last_name}
			/>
			<button class="btn btn-primary" disabled={busy}>Send invitation</button>
		</form>
	</section>

	<section class="card p-5" aria-label="Links to join">
		<h2 class="font-semibold text-slate-900">Links to join</h2>
		<p class="mb-3 text-sm text-slate-500">
			Share a link (in a chat, on the intranet, in an email to a team). People give their name and
			work email, and get the usual invitation to set a password.
		</p>
		<form class="grid gap-3 text-sm sm:grid-cols-2" onsubmit={makeLink}>
			<div>
				<label class="label" for="jl-note">Name (for you)</label><input
					id="jl-note"
					class="input"
					maxlength="200"
					placeholder="New sales team"
					bind:value={form.note}
				/>
			</div>
			<div>
				<label class="label" for="jl-dep">Department</label>
				<select id="jl-dep" class="input" bind:value={form.department_id}
					><option value="">—</option>{#each departments as d (d.id)}<option value={d.id}
							>{d.name}</option
						>{/each}</select
				>
			</div>
			<div>
				<label class="label" for="jl-dom">Only emails at</label><input
					id="jl-dom"
					class="input"
					placeholder="example.com (any if empty)"
					bind:value={form.domains}
				/>
			</div>
			<div class="grid grid-cols-2 gap-2">
				<div>
					<label class="label" for="jl-max">At most people</label><input
						id="jl-max"
						class="input"
						type="number"
						min="1"
						max="10000"
						placeholder="Any"
						bind:value={form.max_uses}
					/>
				</div>
				<div>
					<label class="label" for="jl-exp">Works until</label><input
						id="jl-exp"
						class="input"
						type="date"
						bind:value={form.expires}
					/>
				</div>
			</div>
			<label class="flex items-center gap-2 sm:col-span-2"
				><input
					type="checkbox"
					class="rounded border-slate-300 text-brand-600"
					bind:checked={form.needs_approval}
				/> Someone approves each person before they're invited</label
			>
			<div class="sm:col-span-2">
				<button class="btn btn-primary" disabled={busy}>Make a link</button>
			</div>
		</form>
		{#if madeUrl}
			<p
				class="mt-3 flex flex-wrap items-center gap-2 rounded-lg bg-emerald-50 p-2 text-sm text-emerald-900"
			>
				Link (shown once): <code class="break-all" data-join-link>{madeUrl}</code>
				<button class="text-emerald-800 underline" onclick={() => copy(madeUrl)}>Copy</button>
			</p>
		{/if}
		{#if links.length}
			<table class="mt-4 w-full text-sm">
				<thead
					><tr class="text-left text-xs text-slate-500"
						><th class="py-1">Link</th><th>Rules</th><th>Used</th><th></th></tr
					></thead
				>
				<tbody class="divide-y divide-slate-50">
					{#each links as l (l.id)}
						<tr data-link-row={l.note ?? ''} class={l.usable ? '' : 'text-slate-400'}>
							<td class="py-2"
								>{l.note ?? 'Untitled'}<br /><span class="text-xs text-slate-400"
									>by {l.created_by ?? '—'} · {formatDate(l.created_at)}</span
								></td
							>
							<td class="text-xs"
								>{l.needs_approval ? 'Approval' : 'No approval'}{l.department
									? ` · ${l.department}`
									: ''}{l.email_domains.length
									? ` · @${l.email_domains.join(', @')}`
									: ''}{l.expires_at ? ` · until ${formatDate(l.expires_at)}` : ''}</td
							>
							<td class="text-xs">{l.uses}{l.max_uses ? ` of ${l.max_uses}` : ''}</td>
							<td class="text-right whitespace-nowrap">
								<button class="text-sm text-brand-600 hover:underline" onclick={() => toggle(l)}
									>{l.disabled ? 'Turn on' : 'Turn off'}</button
								>
								<button
									class="ml-2 text-sm text-slate-400 hover:text-red-600"
									aria-label="Delete link"
									onclick={() => drop(l)}>×</button
								>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>
</div>
