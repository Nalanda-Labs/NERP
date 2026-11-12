<script lang="ts">
	// The audit trail's integrity: its hash chain head, the last check, and checking now
	// (optionally against a head noted somewhere else earlier).
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Problem = { position: number; kind: string; text: string };
	type Result = { ok: boolean; records: number; problems: Problem[]; checked_at: string; checkpoint: { position: number; ok: boolean } | null; checkpoints: number };
	type Status = { head: { position: number; hash: string; at: string } | null; unsealed: number; last_checkpoint_at: string | null; last_verify: Result | null };
	let st = $state<Status | null>(null);
	let busy = $state(false);
	let notedPos = $state('');
	let notedHash = $state('');

	async function load() {
		try {
			st = await api.get<Status>('/audit/chain');
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);

	async function verify() {
		busy = true;
		try {
			const body = notedPos && notedHash ? { checkpoint: { position: Number(notedPos), hash: notedHash.trim() } } : {};
			const r = await api.post<Result>('/audit/chain/verify', body);
			if (r.ok) toasts.success(`All ${r.records.toLocaleString()} records check out`);
			else toasts.error(new Error('The audit trail does not verify'));
			await load();
		} catch (e) {
			toasts.error(e);
		} finally {
			busy = false;
		}
	}
	async function checkpoint() {
		try {
			await api.post('/audit/chain/checkpoint', {});
			toasts.success('Checkpoint signed');
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function copy() {
		if (!st?.head) return;
		await navigator.clipboard?.writeText(`${st.head.position} ${st.head.hash}`).catch(() => {});
		toasts.success('Copied');
	}
	const v = $derived(st?.last_verify ?? null);
</script>

<section class="card space-y-3 p-5 text-sm" aria-label="Integrity" id="integrity">
	<div class="flex flex-wrap items-center gap-2">
		<h2 class="font-semibold text-slate-900">Integrity</h2>
		{#if v}
			<span class="rounded-full px-2 py-0.5 text-xs font-medium {v.ok ? 'bg-emerald-50 text-emerald-800' : 'bg-red-50 text-red-800'}" data-integrity={v.ok ? 'ok' : 'broken'}>{v.ok ? 'Verified' : 'Does not verify'}</span>
			<span class="text-xs text-slate-500">{v.records.toLocaleString()} records checked {formatDateTime(v.checked_at)}</span>
		{/if}
		<div class="ml-auto flex gap-2">
			<button class="btn btn-secondary" onclick={checkpoint}>Sign a checkpoint</button>
			<button class="btn btn-primary" disabled={busy} onclick={verify}>{busy ? 'Checking…' : 'Verify now'}</button>
		</div>
	</div>
	<p class="text-slate-500">Each record is chained to the one before it by its hash, and the head is signed hourly with a key that isn't in the database. Records can't be changed or removed; if anything is changed behind the application's back, verifying shows where.</p>
	{#if v && !v.ok}
		<ul class="space-y-1 rounded-lg bg-red-50 p-3 text-red-800" data-problems>
			{#each v.problems as p, i (i)}<li>{p.text}</li>{/each}
		</ul>
	{/if}
	{#if st?.head}
		<div class="flex flex-wrap items-center gap-2 text-xs text-slate-600">
			<span>Head: record <b data-head-position>{st.head.position.toLocaleString()}</b></span>
			<code class="max-w-full truncate rounded bg-slate-100 px-1.5 py-0.5" data-head-hash>{st.head.hash}</code>
			<button class="text-brand-700 hover:underline" onclick={copy}>Copy</button>
			{#if st.last_checkpoint_at}<span class="text-slate-400">· last checkpoint {formatDateTime(st.last_checkpoint_at)}</span>{/if}
			{#if st.unsealed}<span class="text-slate-400">· {st.unsealed} waiting to be chained</span>{/if}
		</div>
		<details class="text-xs text-slate-600">
			<summary class="cursor-pointer text-slate-500">Check a head you noted earlier</summary>
			<p class="mt-1">Note the head (record and hash) somewhere outside NERP from time to time. Entering it here proves the records up to it are still the same.</p>
			<div class="mt-2 flex flex-wrap gap-2">
				<input class="input w-32" placeholder="Record" inputmode="numeric" bind:value={notedPos} aria-label="Noted record" />
				<input class="input min-w-0 flex-1 font-mono" placeholder="Hash" bind:value={notedHash} aria-label="Noted hash" />
				<button class="btn btn-secondary" disabled={busy || !notedPos || !notedHash} onclick={verify}>Check</button>
			</div>
			{#if v?.checkpoint}<p class="mt-1 {v.checkpoint.ok ? 'text-emerald-700' : 'text-red-700'}" data-noted={v.checkpoint.ok ? 'ok' : 'bad'}>{v.checkpoint.ok ? `Record ${v.checkpoint.position} matches.` : `Record ${v.checkpoint.position} doesn't match.`}</p>{/if}
		</details>
	{/if}
</section>
