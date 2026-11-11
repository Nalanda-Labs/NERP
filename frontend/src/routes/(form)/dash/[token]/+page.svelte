<script lang="ts">
	// A BI dashboard opened with a public link (/dash/<token>, view only), with its password if it has one.
	// Visitors pick the period; the dashboard's own filters always apply.
	/* eslint-disable @typescript-eslint/no-explicit-any */
	import { onDestroy, onMount } from 'svelte';
	import { page } from '$app/state';
	import { PERIODS, type DashFilters, type Figures, type Period, type Widget } from '#lib/bi.ts';
	import DashboardGrid from '#lib/components/bi/DashboardGrid.svelte';

	type Opened = {
		title: string;
		description: string | null;
		filters: DashFilters;
		widgets: Widget[];
	};
	const token = $derived(page.params.token!);
	let d = $state<Opened | null>(null);
	let figures = $state<Record<string, Figures> | null>(null);
	let period = $state<Period>({});
	let needPassword = $state(false);
	let password = $state('');
	let error = $state('');
	let missing = $state(false);

	const call = async (method: string, path: string, body?: unknown) => {
		const headers: Record<string, string> = {};
		if (password) headers['X-Dashboard-Password'] = password;
		if (body) headers['content-type'] = 'application/json';
		const r = await fetch(`/api/public/bi/${encodeURIComponent(token)}${path}`, {
			method,
			headers,
			body: body ? JSON.stringify(body) : undefined
		});
		if (!r.ok) {
			const x = await r.json().catch(() => ({}));
			throw Object.assign(new Error(x?.error?.message ?? 'Something went wrong'), {
				status: r.status
			});
		}
		return r.json();
	};
	async function open(e?: SubmitEvent) {
		e?.preventDefault();
		error = '';
		try {
			d = await call('GET', '');
			period = { ...(d!.filters.period ?? {}) };
			needPassword = false;
		} catch (err: any) {
			if (err.status === 401) {
				needPassword = true;
				if (password) error = err.message;
			} else if (err.status === 404) missing = true;
			else error = err.message;
		}
	}
	async function refresh() {
		if (!d) return;
		try {
			const r = await call('POST', '/data', { period: $state.snapshot(period) });
			figures = Object.fromEntries((r.items as Figures[]).map((i) => [i.id, i]));
		} catch (err: any) {
			if (err.status === 404) missing = true;
			else error = err.message;
		}
	}
	$effect(() => {
		JSON.stringify(period);
		if (d) refresh();
	});
	onMount(() => open());
	const timer = setInterval(refresh, 5 * 60_000);
	onDestroy(() => clearInterval(timer));
	const presetKey = $derived(period.from ? 'custom' : (period.preset ?? 'all'));
</script>

<svelte:head><title>{d?.title ?? 'Dashboard'}</title></svelte:head>

{#if missing}
	<main class="flex min-h-dvh items-center justify-center bg-slate-50 p-4">
		<p class="rounded-2xl bg-white p-8 text-slate-600 shadow-sm">This dashboard isn't available.</p>
	</main>
{:else if needPassword}
	<main class="flex min-h-dvh items-center justify-center bg-slate-50 p-4">
		<form class="w-full max-w-sm space-y-3 rounded-2xl bg-white p-6 shadow-sm" onsubmit={open}>
			<h1 class="text-lg font-semibold text-slate-900">This dashboard needs a password</h1>
			<input class="input" type="password" aria-label="Password" required bind:value={password} />
			{#if error}<p class="text-sm text-red-600" role="alert">{error}</p>{/if}
			<button class="btn btn-primary w-full justify-center">Open</button>
		</form>
	</main>
{:else if d}
	<main class="min-h-dvh bg-slate-50 p-4">
		<div class="mx-auto max-w-7xl space-y-3">
			<div class="flex flex-wrap items-center gap-2">
				<div class="min-w-0 flex-1">
					<h1 class="text-2xl font-semibold text-slate-900">{d.title}</h1>
					{#if d.description}<p class="text-sm text-slate-600">{d.description}</p>{/if}
				</div>
				<select
					class="input w-auto py-1 text-sm"
					aria-label="Period"
					value={presetKey}
					onchange={(e) =>
						(period = e.currentTarget.value === 'all' ? {} : { preset: e.currentTarget.value })}
				>
					{#each PERIODS as [k, l] (k)}<option value={k}>{l}</option>{/each}
					{#if presetKey === 'custom'}<option value="custom">{period.from} – {period.to}</option
						>{/if}
				</select>
			</div>
			{#if error}<p class="text-sm text-red-600" role="alert">{error}</p>{/if}
			<DashboardGrid widgets={d.widgets} {figures} />
		</div>
	</main>
{:else if error}
	<main class="flex min-h-dvh items-center justify-center p-4">
		<p class="text-red-600">{error}</p>
	</main>
{/if}
