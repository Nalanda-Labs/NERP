<script lang="ts">
	// Personal API keys: create (the secret shows once), see when they were last used, revoke.
	// People who manage employees can also see and revoke everyone's keys.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';

	type Key = {
		id: string; user_id: string; owner_name: string; name: string; prefix: string; scopes: string[];
		expires_at: string | null; last_used_at: string | null; last_used_ip: string | null; revoked_at: string | null; created_at: string;
	};
	type List = { items: Key[]; scopes: { code: string; label: string }[]; can_create: boolean; can_manage_all: boolean };

	let data = $state<List | null>(null);
	let everyone = $state(false);
	async function load() {
		try {
			data = await api.get<List>(`/api-keys${everyone ? '?all=true' : ''}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);

	let open = $state(false);
	let draft = $state({ name: '', scopes: ['crm:read'] as string[], expires: '' });
	let created = $state<{ key: Key; secret: string } | null>(null);
	let saving = $state(false);
	function newKey() {
		draft = { name: '', scopes: ['crm:read'], expires: '' };
		open = true;
	}
	async function create(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		try {
			created = await api.post<{ key: Key; secret: string }>('/api-keys', { name: draft.name, scopes: draft.scopes, expires_in_days: draft.expires ? Number(draft.expires) : null });
			open = false;
			await load();
		} catch (err) {
			toasts.error(err);
		} finally {
			saving = false;
		}
	}
	async function revoke(k: Key) {
		if (!confirm(`Revoke “${k.name}”? Systems using it stop working at once.`)) return;
		try {
			await api.del(`/api-keys/${k.id}`);
			toasts.success('Key revoked');
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	/** Picking a full scope drops its read-only twin, and the other way round. */
	function toggleScope(code: string, on: boolean) {
		const area = code.split(':')[0];
		draft.scopes = on ? [...draft.scopes.filter((s) => s.split(':')[0] !== area), code] : draft.scopes.filter((s) => s !== code);
	}
	const keyStatus = (k: Key) => (k.revoked_at ? 'Revoked' : k.expires_at && new Date(k.expires_at) < new Date() ? 'Expired' : 'Active');
	const origin = typeof location !== 'undefined' ? location.origin : '';
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<p class="flex-1 text-sm text-slate-600">
			A key acts as the person who made it, with their access rights, limited to the areas you choose. Send it as <code class="rounded bg-slate-100 px-1">Authorization: Bearer nerp_…</code>; each key may make 600 requests a minute.
		</p>
		{#if data?.can_manage_all}
			<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={everyone} onchange={load} /> Everyone's keys</label>
		{/if}
		{#if data?.can_create}<button class="btn btn-primary" onclick={newKey}><Icon name="plus" class="size-4" /> New key</button>{/if}
	</div>
	{#if data && !data.can_create}
		<p class="rounded-xl bg-amber-50 px-4 py-3 text-sm text-amber-900">Making API keys needs the “Use the API” permission; ask an administrator.</p>
	{/if}
	<details class="card px-4 py-3 text-sm" data-mcp-help>
		<summary class="cursor-pointer font-medium text-slate-800">Connect an AI assistant (MCP)</summary>
		<div class="mt-2 space-y-2 text-slate-600">
			<p>AI assistants and agents that speak the Model Context Protocol can work with the CRM, tasks and the employee list through NERP's MCP server, with a key's access. Add a remote (Streamable HTTP) server with:</p>
			<pre class="overflow-x-auto rounded-lg bg-slate-900 px-3 py-2 text-xs text-slate-100" data-mcp-url>{`URL:    ${origin}/api/mcp
Header: Authorization: Bearer nerp_…`}</pre>
			<p>The assistant sees only the tools the key's areas allow, and does only what you could do yourself.</p>
		</div>
	</details>

	<div class="card overflow-x-auto">
		<table class="w-full text-sm">
			<thead class="border-b border-slate-100 text-left text-xs text-slate-500">
				<tr>
					<th class="px-5 py-2.5 font-medium">Name</th>
					{#if everyone}<th class="px-3 py-2.5 font-medium">Owner</th>{/if}
					<th class="px-3 py-2.5 font-medium">Can</th>
					<th class="px-3 py-2.5 font-medium">Last used</th>
					<th class="px-3 py-2.5 font-medium">Expires</th>
					<th class="px-3 py-2.5 font-medium">Status</th>
					<th class="px-5 py-2.5"></th>
				</tr>
			</thead>
			<tbody class="divide-y divide-slate-50">
				{#each data?.items ?? [] as k (k.id)}
					<tr class={k.revoked_at ? 'text-slate-400' : ''} data-key={k.id}>
						<td class="px-5 py-2.5"><p class="font-medium {k.revoked_at ? '' : 'text-slate-900'}">{k.name}</p><p class="font-mono text-xs text-slate-500">{k.prefix}…</p></td>
						{#if everyone}<td class="px-3 py-2.5">{k.owner_name}</td>{/if}
						<td class="px-3 py-2.5"><div class="flex flex-wrap gap-1">{#each k.scopes as sc (sc)}<span class="rounded bg-slate-100 px-1.5 py-0.5 font-mono text-xs text-slate-700">{sc}</span>{/each}</div></td>
						<td class="px-3 py-2.5 whitespace-nowrap">{k.last_used_at ? formatRelative(k.last_used_at) : 'never'}{#if k.last_used_ip}<span class="block text-xs text-slate-400">{k.last_used_ip}</span>{/if}</td>
						<td class="px-3 py-2.5 whitespace-nowrap">{k.expires_at ? formatDateTime(k.expires_at) : 'never'}</td>
						<td class="px-3 py-2.5"><span class="rounded-full px-2 py-0.5 text-xs font-medium {keyStatus(k) === 'Active' ? 'bg-emerald-50 text-emerald-800' : 'bg-slate-100 text-slate-600'}">{keyStatus(k)}</span></td>
						<td class="px-5 py-2.5 text-right">{#if !k.revoked_at}<button class="btn btn-ghost px-2 py-1 text-xs text-red-600" onclick={() => revoke(k)}>Revoke</button>{/if}</td>
					</tr>
				{:else}
					<tr><td colspan="7" class="px-5 py-8 text-center text-slate-500">{data ? 'No API keys yet.' : 'Loading…'}</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>

<Modal bind:open title="New API key" size="md">
	<form id="key-form" class="space-y-4 text-sm" onsubmit={create}>
		<div><label class="label" for="k-name">Name</label><input id="k-name" class="input" required maxlength="100" bind:value={draft.name} placeholder="e.g. Accounting sync" /></div>
		<fieldset>
			<legend class="label">What it may do</legend>
			<div class="space-y-1.5">
				{#each data?.scopes ?? [] as sc (sc.code)}
					<label class="flex items-start gap-2"><input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" checked={draft.scopes.includes(sc.code)} onchange={(e) => toggleScope(sc.code, e.currentTarget.checked)} /><span><code class="text-xs">{sc.code}</code> — {sc.label}</span></label>
				{/each}
			</div>
		</fieldset>
		<div>
			<label class="label" for="k-exp">Expires</label>
			<select id="k-exp" class="input" bind:value={draft.expires}><option value="">Never (until revoked)</option><option value="30">In 30 days</option><option value="90">In 90 days</option><option value="365">In a year</option></select>
		</div>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" type="submit" form="key-form" disabled={saving || !draft.scopes.length}>{saving ? 'Creating…' : 'Create key'}</button>
	{/snippet}
</Modal>

{#if created}
	{@const c = created}
	<Modal open={true} title="Your new key" size="md" onclose={() => (created = null)}>
		<div class="space-y-3 text-sm">
			<p class="text-slate-600">Copy it now: it is shown only once. Anyone with it can act as you within its scopes.</p>
			<div class="flex gap-2">
				<code class="min-w-0 flex-1 rounded-lg bg-slate-100 px-3 py-2 font-mono text-xs break-all" data-secret>{c.secret}</code>
				<button class="btn btn-secondary" onclick={() => navigator.clipboard?.writeText(c.secret).then(() => toasts.success('Copied'))}>Copy</button>
			</div>
			<p class="text-slate-600">Try it:</p>
			<pre class="overflow-x-auto rounded-lg bg-slate-900 px-3 py-2 text-xs text-slate-100">curl -H "Authorization: Bearer {c.secret}" \
  {origin}/api/crm/deal/items?limit=5</pre>
			<p class="text-slate-600">For an AI assistant (MCP): <code class="rounded bg-slate-100 px-1">{origin}/api/mcp</code> with the same header.</p>
		</div>
		{#snippet footer()}<button class="btn btn-primary" onclick={() => (created = null)}>Done</button>{/snippet}
	</Modal>
{/if}
