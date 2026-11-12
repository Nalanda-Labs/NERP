<script lang="ts">
	// Retention rules: how long each kind of data is kept and what happens after.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Rule = { key: string; title: string; detail: string; actions: string[]; min_days: number; default_days: number; enabled: boolean; keep_days: number; action: string; due_now: number; last_run: { at: string; affected: number; error: string | null } | null };
	type Log = { key: string; action: string; keep_days: number; affected: number; error: string | null; at: string; by: string | null };
	let rules = $state<Rule[]>([]);
	let log = $state<Log[]>([]);
	let kept = $state<string[]>([]);
	let running = $state(false);

	async function load() {
		try {
			const r = await api.get<{ rules: Rule[]; log: Log[]; always_kept: string[] }>('/admin/retention');
			rules = r.rules;
			log = r.log;
			kept = r.always_kept;
		} catch (e) {
			toasts.error(e);
		}
	}
	onMount(load);
	async function save(r: Rule) {
		try {
			const x = await api.put<{ due_now: number }>(`/admin/retention/${r.key}`, { enabled: r.enabled, keep_days: Number(r.keep_days), action: r.action });
			r.due_now = x.due_now;
			toasts.success(`${r.title}: saved`);
		} catch (e) {
			toasts.error(e);
			await load();
		}
	}
	async function runNow() {
		running = true;
		try {
			const x = await api.post<{ results: { key: string; affected: number }[] }>('/admin/retention/run', {});
			const n = x.results.reduce((a, b) => a + b.affected, 0);
			toasts.success(n ? `${n} records handled` : 'Nothing was due');
			await load();
		} catch (e) {
			toasts.error(e);
		} finally {
			running = false;
		}
	}
	const title = (k: string) => rules.find((r) => r.key === k)?.title ?? k;
	const ACT: Record<string, string> = { delete: 'Delete', anonymise: 'Anonymise' };
</script>

<svelte:head><title>Retention · NERP</title></svelte:head>

<div class="max-w-5xl space-y-5">
	<div class="flex flex-wrap items-center gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Retention rules</h1>
			<p class="text-sm text-slate-500"><a class="text-brand-700 hover:underline" href="/privacy">Data requests</a> · how long each kind of data is kept; applied every night, a batch at a time.</p>
		</div>
		<button class="btn btn-primary ml-auto" disabled={running} onclick={runNow} data-run-retention>{running ? 'Applying…' : 'Apply now'}</button>
	</div>

	<section class="card divide-y divide-slate-100" aria-label="Rules">
		{#each rules as r (r.key)}
			<div class="flex flex-wrap items-start gap-3 px-5 py-3 text-sm" data-rule={r.key}>
				<label class="mt-0.5 flex items-center"><input type="checkbox" bind:checked={r.enabled} onchange={() => save(r)} aria-label="Apply {r.title}" data-enabled /></label>
				<div class="min-w-0 flex-1">
					<p class="font-medium text-slate-900">{r.title}</p>
					<p class="text-xs text-slate-500">{r.detail}</p>
					<p class="mt-0.5 text-xs {r.due_now ? 'text-amber-700' : 'text-slate-400'}" data-due>{r.due_now ? `${r.due_now.toLocaleString()}${r.due_now >= 100000 ? '+' : ''} records are older than this now` : 'Nothing older than this now'}{r.last_run ? ` · last run ${formatDateTime(r.last_run.at)}: ${r.last_run.error ? 'failed' : r.last_run.affected}` : ''}</p>
				</div>
				<label class="flex items-center gap-1.5">Keep <input class="input w-24" type="number" min={r.min_days} bind:value={r.keep_days} onchange={() => save(r)} aria-label="{r.title}: days" data-days /> days, then</label>
				<select class="input w-32" bind:value={r.action} onchange={() => save(r)} disabled={r.actions.length === 1} aria-label="{r.title}: action">
					{#each r.actions as a (a)}<option value={a}>{ACT[a]}</option>{/each}
				</select>
			</div>
		{/each}
	</section>

	{#if kept.length}
		<section class="card p-5 text-sm" aria-label="Always kept">
			<h2 class="font-semibold text-slate-900">Always kept</h2>
			<ul class="mt-1 list-disc pl-5 text-slate-600">{#each kept as k (k)}<li>{k}</li>{/each}</ul>
		</section>
	{/if}

	<section class="card p-5 text-sm" aria-label="Log">
		<h2 class="font-semibold text-slate-900">What the rules did</h2>
		<ul class="mt-2 space-y-1" data-log>
			{#each log as l, i (i)}
				<li class="text-slate-700">{formatDateTime(l.at)} · <b class="font-medium">{title(l.key)}</b> ({l.keep_days} days): {l.error ? `failed — ${l.error}` : `${ACT[l.action] ?? l.action} ${l.affected}`}{l.by ? ` · run by ${l.by}` : ''}</li>
			{:else}<li class="text-slate-500">Nothing yet.</li>{/each}
		</ul>
	</section>
</div>
