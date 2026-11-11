<script lang="ts">
	// One BI dashboard: its charts for a period and filters (saved as presets), and for BI
	// builders, changing it: charts, layout, default filters, sharing and public links.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onDestroy } from 'svelte';
	import { publicLinksAllowed } from '#lib/companyRules.ts';
	import { api } from '#lib/api.ts';
	import {
		complete,
		filterLabel,
		PERIODS,
		periodLabel,
		type Col,
		type Dataset,
		type DashFilters,
		type Figures,
		type Filter,
		type Period,
		type Spec,
		type Widget
	} from '#lib/bi.ts';
	import { formatDate } from '#lib/format.ts';
	import Modal from '#lib/components/Modal.svelte';
	import UserPicker from '#lib/components/UserPicker.svelte';
	import DashboardGrid from '#lib/components/bi/DashboardGrid.svelte';
	import FilterList from '#lib/components/bi/FilterList.svelte';
	import WidgetEditor from '#lib/components/bi/WidgetEditor.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type Preset = { id: string; name: string; shared: boolean; filters: DashFilters; mine: boolean };
	type Grant = { kind: 'everyone' | 'department' | 'user'; id: string | null; name: string | null };
	type Dash = {
		id: string;
		title: string;
		description: string | null;
		owner: string | null;
		filters: DashFilters;
		can_edit: boolean;
		widgets: Widget[];
		fields: Col[];
		presets: Preset[];
		access?: Grant[];
	};
	const id = $derived(page.params.id!);
	let d = $state<Dash | null>(null);
	let figures = $state<Record<string, Figures> | null>(null);
	let shownPeriod = $state<{ from: string; to: string } | null>(null);
	let period = $state<Period>({});
	let filters = $state<Filter[]>([]);
	let editing = $state(page.url.searchParams.get('edit') === '1');
	let datasets = $state<Dataset[]>([]);
	let editorOpen = $state(false);
	let editingWidget = $state<Widget | null>(null);
	let filtersOpen = $state(false);
	let custom = $state({ from: '', to: '' });

	function apply(f: DashFilters) {
		period = { ...(f.period ?? {}) };
		filters = (f.filters ?? []).map((x) => ({ ...x }));
		custom = { from: period.from ?? '', to: period.to ?? '' };
	}
	async function load() {
		try {
			d = await api.get<Dash>(`/bi/dashboards/${id}`);
			apply(d.filters);
			if (!d.can_edit) editing = false;
			if (editing) await loadDatasets();
		} catch (e) {
			toasts.error(e);
		}
	}
	async function loadDatasets() {
		if (!datasets.length) datasets = (await api.get<{ items: Dataset[] }>('/bi/datasets')).items;
	}
	let seq = 0;
	async function refresh() {
		if (!d) return;
		const mine = ++seq;
		try {
			const r = await api.post<{ items: Figures[]; period: { from: string; to: string } | null }>(
				`/bi/dashboards/${id}/data`,
				{ period: $state.snapshot(period), filters: complete($state.snapshot(filters)) }
			);
			if (mine !== seq) return;
			figures = Object.fromEntries(r.items.map((i) => [i.id, i]));
			shownPeriod = r.period;
		} catch (e) {
			toasts.error(e);
		}
	}
	$effect(() => {
		id;
		load();
	});
	$effect(() => {
		// Figures again whenever the charts, the period or the filters change.
		JSON.stringify([d?.widgets, period, filters]);
		refresh();
	});
	const timer = setInterval(refresh, 5 * 60_000);
	onDestroy(() => clearInterval(timer));

	function setPreset(p: string) {
		period =
			p === 'all'
				? {}
				: p === 'custom'
					? {
							from: custom.from || new Date().toISOString().slice(0, 10),
							to: custom.to || new Date().toISOString().slice(0, 10)
						}
					: { preset: p };
	}
	const presetKey = $derived(period.from ? 'custom' : (period.preset ?? 'all'));

	// Editing.
	async function startEditing() {
		await loadDatasets();
		editing = true;
	}
	function newWidget() {
		editingWidget = null;
		editorOpen = true;
	}
	async function saveWidget(w: { title: string; chart: string; w: number; h: number; spec: Spec }) {
		try {
			d = editingWidget
				? await api.patch<Dash>(`/bi/widgets/${editingWidget.id}`, w)
				: await api.post<Dash>(`/bi/dashboards/${id}/widgets`, w);
		} catch (e) {
			toasts.error(e);
			throw e;
		}
	}
	async function removeWidget(w: Widget) {
		if (confirm(`Remove “${w.title}”?`)) d = await api.del<Dash>(`/bi/widgets/${w.id}`);
	}
	async function layout(ws: Widget[]) {
		try {
			d = await api.put<Dash>(`/bi/dashboards/${id}/layout`, {
				items: ws.map((w) => ({ id: w.id, w: w.w, h: w.h }))
			});
		} catch (e) {
			toasts.error(e);
		}
	}
	function move(w: Widget, by: number) {
		const ws = [...d!.widgets];
		const i = ws.findIndex((x) => x.id === w.id);
		[ws[i], ws[i + by]] = [ws[i + by], ws[i]];
		layout(ws);
	}
	function resize(w: Widget, dw: number, dh: number) {
		layout(
			d!.widgets.map((x) =>
				x.id === w.id
					? { ...x, w: Math.min(12, Math.max(2, x.w + dw)), h: Math.min(6, Math.max(1, x.h + dh)) }
					: x
			)
		);
	}
	async function rename(field: 'title' | 'description', value: string) {
		if (!d || (d[field] ?? '') === value) return;
		try {
			d = await api.patch<Dash>(`/bi/dashboards/${id}`, { [field]: value });
		} catch (e) {
			toasts.error(e);
		}
	}
	async function saveDefaults() {
		try {
			d = await api.patch<Dash>(`/bi/dashboards/${id}`, {
				filters: { period: $state.snapshot(period), filters: complete($state.snapshot(filters)) }
			});
			toasts.success('These are now the dashboard’s period and filters');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function copy() {
		const c = await api.post<{ id: string }>(`/bi/dashboards/${id}/copy`);
		goto(`/bi/${c.id}?edit=1`);
	}
	async function remove() {
		if (!confirm('Delete this dashboard and its charts?')) return;
		await api.del(`/bi/dashboards/${id}`);
		goto('/bi');
	}

	// Presets.
	let presetOpen = $state(false);
	let presetName = $state('');
	let presetShared = $state(false);
	async function savePreset(e: SubmitEvent) {
		e.preventDefault();
		try {
			d = await api.post<Dash>(`/bi/dashboards/${id}/presets`, {
				name: presetName,
				shared: presetShared,
				filters: { period: $state.snapshot(period), filters: complete($state.snapshot(filters)) }
			});
			presetOpen = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function dropPreset(p: Preset) {
		if (confirm(`Delete the preset “${p.name}”?`)) d = await api.del<Dash>(`/bi/presets/${p.id}`);
	}

	// Sharing.
	let shareOpen = $state(false);
	let grants = $state<Grant[]>([]);
	let departments = $state<{ id: string; name: string }[]>([]);
	let links = $state<
		{ id: string; expires_at: string | null; password: boolean; created_by: string | null }[]
	>([]);
	let newLink = $state({ expires: '', password: '' });
	let madeUrl = $state('');
	let linksOn = $state(true);
	publicLinksAllowed().then((v) => (linksOn = v));
	async function openShare() {
		grants = (d?.access ?? []).map((g) => ({ ...g }));
		madeUrl = '';
		shareOpen = true;
		departments = await api.get<{ id: string; name: string }[]>('/departments').catch(() => []);
		links = (await api.get<{ items: typeof links }>(`/bi/dashboards/${id}/links`)).items;
	}
	function addGrant(kind: Grant['kind'], gid: string | null, name: string | null) {
		if (!grants.some((g) => g.kind === kind && g.id === gid))
			grants = [...grants, { kind, id: gid, name }];
	}
	async function saveAccess() {
		try {
			const r = await api.put<{ access: Grant[] }>(`/bi/dashboards/${id}/access`, {
				grants: grants.map(({ kind, id: gid }) => ({ kind, id: gid }))
			});
			d = { ...d!, access: r.access };
			toasts.success('Sharing saved');
		} catch (e) {
			toasts.error(e);
		}
	}
	async function makeLink() {
		try {
			const r = await api.post<{ url: string }>(`/bi/dashboards/${id}/links`, {
				expires_at: newLink.expires ? new Date(`${newLink.expires}T23:59:59`).toISOString() : null,
				password: newLink.password || null
			});
			madeUrl = r.url;
			newLink = { expires: '', password: '' };
			links = (await api.get<{ items: typeof links }>(`/bi/dashboards/${id}/links`)).items;
		} catch (e) {
			toasts.error(e);
		}
	}
	async function dropLink(lid: string) {
		await api.del(`/bi/dashboards/${id}/links/${lid}`);
		links = links.filter((l) => l.id !== lid);
	}
</script>

<svelte:head><title>{d?.title ?? 'Dashboard'} · NERP</title></svelte:head>

{#if d}
	<div class="space-y-3">
		<div class="flex flex-wrap items-start gap-2">
			<a class="btn btn-ghost px-2" href="/bi" aria-label="All dashboards">‹</a>
			<div class="min-w-0 flex-1">
				{#if editing}
					<input
						class="input py-1 text-lg font-semibold"
						aria-label="Dashboard name"
						maxlength="200"
						value={d.title}
						onblur={(e) => rename('title', e.currentTarget.value)}
					/>
					<input
						class="input mt-1 py-1 text-sm"
						aria-label="What it shows"
						placeholder="What it shows (optional)"
						value={d.description ?? ''}
						onblur={(e) => rename('description', e.currentTarget.value)}
					/>
				{:else}
					<h1 class="text-2xl font-semibold text-slate-900">{d.title}</h1>
					{#if d.description}<p class="text-sm text-slate-600">{d.description}</p>{/if}
				{/if}
			</div>
			{#if d.can_edit}
				<div class="flex flex-wrap gap-2">
					{#if editing}
						<button class="btn btn-primary" onclick={newWidget}>+ Chart</button>
						<button
							class="btn btn-secondary"
							onclick={saveDefaults}
							title="Open the dashboard with this period and these filters"
							>Keep these filters</button
						>
						<button class="btn btn-secondary" onclick={() => (editing = false)}>Done</button>
					{:else}
						<button class="btn btn-secondary" onclick={startEditing}>Edit</button>
					{/if}
					<button class="btn btn-secondary" onclick={openShare}>Share</button>
					<button class="btn btn-ghost" onclick={copy}>Copy</button>
					<button class="btn btn-ghost hover:text-red-600" onclick={remove}>Delete</button>
				</div>
			{/if}
		</div>

		<div class="flex flex-wrap items-center gap-2 text-sm" aria-label="Period and filters">
			<select
				class="input w-auto py-1"
				aria-label="Period"
				value={presetKey}
				onchange={(e) => setPreset(e.currentTarget.value)}
			>
				{#each PERIODS as [k, l] (k)}<option value={k}>{l}</option>{/each}
				<option value="custom">Dates…</option>
			</select>
			{#if presetKey === 'custom'}
				<input
					class="input w-auto py-1"
					type="date"
					aria-label="From"
					bind:value={custom.from}
					onchange={() => (period = { from: custom.from, to: custom.to || custom.from })}
				/>
				<span class="text-slate-400">–</span>
				<input
					class="input w-auto py-1"
					type="date"
					aria-label="To"
					bind:value={custom.to}
					onchange={() => (period = { from: custom.from || custom.to, to: custom.to })}
				/>
			{/if}
			{#if shownPeriod}<span class="text-xs text-slate-500" data-period
					>{formatDate(shownPeriod.from)} – {formatDate(shownPeriod.to)}</span
				>{/if}
			{#each filters as f, i (i)}
				<span class="badge text-brand-800 bg-brand-50"
					>{filterLabel(f, d.fields)}
					<button
						class="ml-1"
						aria-label="Remove filter"
						onclick={() => (filters = filters.filter((_, j) => j !== i))}>×</button
					></span
				>
			{/each}
			<button
				class="btn btn-ghost py-1"
				onclick={() => (filtersOpen = true)}
				disabled={!d.fields.length}>Filters…</button
			>
			<select
				class="input w-auto py-1"
				aria-label="Presets"
				value=""
				onchange={(e) => {
					const v = e.currentTarget.value;
					const p = d!.presets.find((x) => x.id === v);
					if (p) apply(p.filters);
					else if (v === '__default') apply(d!.filters);
					e.currentTarget.value = '';
				}}
			>
				<option value="">Presets…</option>
				<option value="__default">The dashboard’s own</option>
				{#each d.presets as p (p.id)}<option value={p.id}
						>{p.name}{p.shared ? ' (everyone)' : ''}</option
					>{/each}
			</select>
			<button
				class="btn btn-ghost py-1"
				onclick={() => ((presetName = ''), (presetShared = false), (presetOpen = true))}
				>Save as preset</button
			>
			<button class="btn btn-ghost ml-auto py-1" onclick={refresh} title="Figures again">↻</button>
		</div>

		<DashboardGrid
			widgets={d.widgets}
			{figures}
			{editing}
			onedit={async (w) => {
				await loadDatasets();
				editingWidget = w;
				editorOpen = true;
			}}
			onremove={removeWidget}
			onmove={move}
			onresize={resize}
		/>
	</div>

	<WidgetEditor
		bind:open={editorOpen}
		{datasets}
		widget={editingWidget}
		{period}
		onsave={saveWidget}
	/>

	<Modal bind:open={filtersOpen} title="Filters for every chart">
		<div class="space-y-2 text-sm">
			<p class="text-slate-500">Each condition applies to the charts whose data has that column.</p>
			<FilterList cols={d.fields} bind:filters label="Only rows where" />
		</div>
		{#snippet footer()}<button class="btn btn-primary" onclick={() => (filtersOpen = false)}
				>Done</button
			>{/snippet}
	</Modal>

	<Modal bind:open={presetOpen} title="Save as preset" size="sm">
		<form id="bi-preset" class="space-y-3 text-sm" onsubmit={savePreset}>
			<p class="text-slate-500">
				{periodLabel(period)}{filters.length
					? ` · ${filters.map((f) => filterLabel(f, d!.fields)).join(' · ')}`
					: ''}
			</p>
			<div>
				<label class="label" for="bp-name">Name</label><input
					id="bp-name"
					class="input"
					required
					maxlength="100"
					bind:value={presetName}
				/>
			</div>
			{#if d.can_edit}<label class="flex items-center gap-2"
					><input type="checkbox" class="rounded border-slate-300" bind:checked={presetShared} /> For
					everyone who opens the dashboard</label
				>{/if}
			{#if d.presets.some((p) => p.mine || d!.can_edit)}
				<div class="border-t border-slate-100 pt-2">
					<p class="label">Saved presets</p>
					{#each d.presets.filter((p) => p.mine || d!.can_edit) as p (p.id)}<p
							class="flex items-center gap-2"
						>
							<span class="flex-1">{p.name}{p.shared ? ' (everyone)' : ''}</span><button
								type="button"
								class="text-red-600 hover:underline"
								onclick={() => dropPreset(p)}>Delete</button
							>
						</p>{/each}
				</div>
			{/if}
		</form>
		{#snippet footer()}
			<button class="btn btn-secondary" onclick={() => (presetOpen = false)}>Cancel</button>
			<button class="btn btn-primary" form="bi-preset">Save</button>
		{/snippet}
	</Modal>

	<Modal bind:open={shareOpen} title="Share the dashboard">
		<div class="space-y-4 text-sm">
			<p class="rounded-lg bg-amber-50 p-2 text-amber-900">
				Everyone you share it with sees its figures, whatever their own access to the records behind
				them.
			</p>
			<div class="space-y-2">
				<span class="label">People in the company (they can view it)</span>
				{#each grants as g, i (g.kind + g.id)}
					<div class="flex items-center gap-2" data-grant>
						<span class="flex-1 truncate"
							>{g.kind === 'everyone'
								? 'Everyone'
								: g.kind === 'department'
									? `🏢 ${g.name}`
									: g.name}</span
						>
						<button
							class="px-1 text-slate-400 hover:text-red-600"
							aria-label="Remove"
							onclick={() => (grants = grants.filter((_, j) => j !== i))}>×</button
						>
					</div>
				{/each}
				<div class="grid gap-2 sm:grid-cols-3">
					<UserPicker
						placeholder="Add a person…"
						onpick={(u) => u && addGrant('user', u.id, u.full_name)}
					/>
					<select
						class="input"
						aria-label="Add a department"
						onchange={(e) => {
							const x = departments.find((y) => y.id === e.currentTarget.value);
							if (x) addGrant('department', x.id, x.name);
							e.currentTarget.value = '';
						}}
					>
						<option value="">Add a department…</option>{#each departments as x (x.id)}<option
								value={x.id}>{x.name}</option
							>{/each}
					</select>
					<button class="btn btn-secondary" onclick={() => addGrant('everyone', null, null)}
						>Everyone</button
					>
				</div>
				<button class="btn btn-primary" onclick={saveAccess}>Save</button>
			</div>
			<div class="space-y-2 border-t border-slate-100 pt-3">
				<span class="label">Links for anyone (also outside the company; view only)</span>
				{#each links as l (l.id)}
					<div class="flex items-center gap-2 text-slate-600" data-link>
						<span class="flex-1"
							>Made by {l.created_by ?? '—'}{l.expires_at
								? ` · until ${new Date(l.expires_at).toLocaleDateString()}`
								: ''}{l.password ? ' · password' : ''}</span
						>
						<button class="text-red-600 hover:underline" onclick={() => dropLink(l.id)}
							>Turn off</button
						>
					</div>
				{/each}
				{#if madeUrl}<p class="rounded-lg bg-emerald-50 p-2 text-emerald-900">
						Link (shown once): <code class="break-all" data-dashboard-link>{madeUrl}</code>
					</p>{/if}
				{#if linksOn}
				<div class="grid gap-2 sm:grid-cols-3">
					<input class="input" type="date" aria-label="Works until" bind:value={newLink.expires} />
					<input
						class="input"
						type="text"
						aria-label="Link password"
						placeholder="Password (optional)"
						autocomplete="off"
						bind:value={newLink.password}
					/>
					<button class="btn btn-secondary" onclick={makeLink}>Make a link</button>
				</div>
				{:else}<p class="text-xs text-amber-800" data-links-off>Links for anyone are switched off for the company.</p>{/if}
			</div>
		</div>
		{#snippet footer()}<button class="btn btn-secondary" onclick={() => (shareOpen = false)}
				>Close</button
			>{/snippet}
	</Modal>
{/if}
