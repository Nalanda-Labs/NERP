<script lang="ts">
	// Marketing → Traffic: where clients come from.
	// * Analysis: visits to the company's websites by channel or source, and what those visitors
	//   became (records, deals, wins, revenue) against what was spent.
	// * Websites: the tracking script for each site.
	// * Ad links: short links and QR codes for offline ads, with who came through them.
	// * Spend: what each channel or source cost per month.
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { replaceState } from '$app/navigation';
	import { api, query, requestBlob } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { crm, money } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Row = { key: string; visits: number; visitors: number; new_visitors: number; records: number; deals: number; won: number; revenue: string; spend: string; conversion: number | null; cost_per_record: string | null; roi: string | null };
	type Report = { by: 'channel' | 'source'; base_currency: string; rows: Row[]; total: Row };
	type Site = { id: string; name: string; domains: string[]; key: string; is_active: boolean; snippet: string; visits_30d: number };
	type Link = { id: string; code: string; name: string; target_url: string; source: string; kind: string; clicks: number; visits: number; visitors: number; records: number; short_url: string; is_active: boolean };
	type Spend = { source: string; month: string; amount: string; currency: string; amount_base: string; note: string | null };

	const TABS = [['analysis', 'Analysis'], ['sites', 'Websites'], ['links', 'Ad links'], ['spend', 'Spend']] as const;
	type Tab = (typeof TABS)[number][0];
	let tab = $state<Tab>((TABS.find(([k]) => k === page.url.searchParams.get('tab'))?.[0] ?? 'analysis') as Tab);
	function setTab(t: Tab) {
		tab = t;
		const u = new URL(page.url.href);
		u.searchParams.set('tab', t);
		replaceState(u, {});
	}
	const CHANNEL: Record<string, string> = { paid: 'Paid ads', search: 'Search engines', social: 'Social networks', email: 'Email', referral: 'Other websites', campaign: 'Tagged campaigns', offline: 'Offline ads', direct: 'Direct' };

	// ---- analysis ----
	const iso = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
	const now = new Date();
	let from = $state(iso(new Date(now.getFullYear(), now.getMonth(), 1)));
	let to = $state(iso(now));
	let by = $state<'channel' | 'source'>('channel');
	let siteFilter = $state('');
	let report = $state<Report | null>(null);
	async function loadReport() {
		try {
			report = await api.get<Report>('/tracking/report' + query({ from, to, by, site_id: siteFilter || undefined }));
		} catch (e) {
			toasts.error(e);
		}
	}
	const label = (k: string) => (by === 'channel' ? (CHANNEL[k] ?? k) : k);

	// ---- sites ----
	let sites = $state<Site[]>([]);
	let newSite = $state({ name: '', domains: '' });
	const loadSites = async () => (sites = (await api.get<{ items: Site[] }>('/tracking/sites')).items);
	async function addSite(e: SubmitEvent) {
		e.preventDefault();
		try {
			await api.post('/tracking/sites', { name: newSite.name, domains: newSite.domains.split(/[\s,]+/).filter(Boolean) });
			newSite = { name: '', domains: '' };
			await loadSites();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeSite(s: Site) {
		if (!confirm(`Remove “${s.name}” and its visit history?`)) return;
		await api.del(`/tracking/sites/${s.id}`).catch(toasts.error);
		await loadSites();
	}
	async function copy(text: string) {
		try {
			await navigator.clipboard.writeText(text);
			toasts.success('Copied');
		} catch {
			toasts.error('Select the text and copy it');
		}
	}

	// ---- links ----
	let links = $state<Link[]>([]);
	let newLink = $state({ name: '', target_url: '', source: '', campaign: '', kind: 'offline' });
	const loadLinks = async () => (links = (await api.get<{ items: Link[] }>('/tracking/links')).items);
	async function addLink(e: SubmitEvent) {
		e.preventDefault();
		try {
			await api.post('/tracking/links', { ...newLink, campaign: newLink.campaign || undefined });
			newLink = { name: '', target_url: '', source: '', campaign: '', kind: 'offline' };
			await loadLinks();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function toggleLink(l: Link) {
		await api.patch(`/tracking/links/${l.id}`, { is_active: !l.is_active }).catch(toasts.error);
		await loadLinks();
	}
	async function qr(l: Link) {
		try {
			const blob = await requestBlob('GET', `/tracking/links/${l.id}/qr.svg`);
			const a = document.createElement('a');
			a.href = URL.createObjectURL(blob);
			a.download = `qr-${l.code}.svg`;
			a.click();
			setTimeout(() => URL.revokeObjectURL(a.href), 5000);
		} catch (e) {
			toasts.error(e);
		}
	}

	// ---- spend ----
	let year = $state(now.getFullYear());
	let spend = $state<Spend[]>([]);
	let base = $state('');
	let newSpend = $state({ source: 'paid', month: `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}`, amount: '', currency: '', note: '' });
	async function loadSpend() {
		const r = await api.get<{ items: Spend[]; base_currency: string }>('/tracking/spend' + query({ year }));
		spend = r.items;
		base = r.base_currency;
	}
	async function saveSpend(e: SubmitEvent) {
		e.preventDefault();
		try {
			await api.put('/tracking/spend', { ...newSpend, currency: newSpend.currency || undefined, note: newSpend.note || undefined });
			newSpend = { ...newSpend, amount: '', note: '' };
			await loadSpend();
		} catch (err) {
			toasts.error(err);
		}
	}
	async function removeSpend(s: Spend) {
		await api.put('/tracking/spend', { source: s.source, month: s.month, amount: '' }).catch(toasts.error);
		await loadSpend();
	}

	onMount(() => {
		crm.load();
		loadReport();
		loadSites();
		loadLinks();
		loadSpend();
	});
	const sources = $derived(Object.entries(crm.config?.source_labels ?? {}).map(([code, name]) => ({ code, name })));
</script>

<svelte:head><title>Traffic · NERP</title></svelte:head>

<div class="space-y-4">
	<div class="flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold text-slate-900">Traffic</h1>
		<div class="ml-auto flex rounded-full bg-slate-100 p-0.5 text-sm" role="tablist" aria-label="Traffic view">
			{#each TABS as [k, l] (k)}
				<button role="tab" aria-selected={tab === k} class="rounded-full px-3 py-1 {tab === k ? 'bg-white font-medium text-slate-900 shadow-g1' : 'text-slate-600 hover:text-slate-900'}" onclick={() => setTab(k)}>{l}</button>
			{/each}
		</div>
	</div>

	{#if tab === 'analysis'}
		<div class="card flex flex-wrap items-end gap-3 p-3 text-sm">
			<div><label class="label" for="t-from">From</label><input id="t-from" type="date" class="input" bind:value={from} onchange={loadReport} /></div>
			<div><label class="label" for="t-to">To</label><input id="t-to" type="date" class="input" bind:value={to} onchange={loadReport} /></div>
			<div>
				<label class="label" for="t-site">Website</label>
				<select id="t-site" class="input w-48" bind:value={siteFilter} onchange={loadReport}><option value="">All websites</option>{#each sites as s (s.id)}<option value={s.id}>{s.name}</option>{/each}</select>
			</div>
			<div class="ml-auto flex rounded-full bg-slate-100 p-0.5 text-xs" role="tablist" aria-label="Group by">
				{#each [['channel', 'Channels'], ['source', 'Sources']] as const as [k, l] (k)}
					<button role="tab" aria-selected={by === k} class="rounded-full px-3 py-1 {by === k ? 'bg-white font-medium text-slate-900 shadow-sm' : 'text-slate-600'}" onclick={() => ((by = k), loadReport())}>{l}</button>
				{/each}
			</div>
		</div>
		{#if report}
			{@const cur = report.base_currency}
			<section class="card overflow-x-auto" data-traffic-report>
				<table class="min-w-full text-sm tabular-nums">
					<thead class="text-left text-xs text-slate-500">
						<tr>
							<th class="px-4 py-2 font-medium">{by === 'channel' ? 'Channel' : 'Source'}</th>
							<th class="px-3 py-2 text-right font-medium">Visits</th><th class="px-3 py-2 text-right font-medium">Visitors</th><th class="px-3 py-2 text-right font-medium">New</th>
							<th class="px-3 py-2 text-right font-medium" title="CRM records made by visitors who first came this way">Records</th><th class="px-3 py-2 text-right font-medium">Conversion</th>
							<th class="px-3 py-2 text-right font-medium">Deals</th><th class="px-3 py-2 text-right font-medium">Won</th><th class="px-3 py-2 text-right font-medium">Revenue</th>
							<th class="px-3 py-2 text-right font-medium">Spend</th><th class="px-3 py-2 text-right font-medium">Per record</th><th class="px-4 py-2 text-right font-medium">ROI</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-slate-50">
						{#each report.rows as r (r.key)}
							<tr data-channel={r.key}>
								<td class="px-4 py-2 text-slate-800">{label(r.key)}</td>
								<td class="px-3 py-2 text-right">{r.visits}</td><td class="px-3 py-2 text-right">{r.visitors}</td><td class="px-3 py-2 text-right text-slate-500">{r.new_visitors}</td>
								<td class="px-3 py-2 text-right">{r.records}</td><td class="px-3 py-2 text-right text-slate-500">{r.conversion === null ? '—' : `${r.conversion}%`}</td>
								<td class="px-3 py-2 text-right">{r.deals}</td><td class="px-3 py-2 text-right">{r.won}</td><td class="px-3 py-2 text-right">{money(r.revenue, cur)}</td>
								<td class="px-3 py-2 text-right">{Number(r.spend) ? money(r.spend, cur) : '—'}</td><td class="px-3 py-2 text-right">{r.cost_per_record ? money(r.cost_per_record, cur) : '—'}</td>
								<td class="px-4 py-2 text-right {r.roi && Number(r.roi) < 0 ? 'text-red-700' : ''}">{r.roi === null ? '—' : `${r.roi}%`}</td>
							</tr>
						{:else}
							<tr><td colspan="12" class="px-4 py-10 text-center text-slate-500">No visits in this period. Add the tracking script to your websites under “Websites”.</td></tr>
						{/each}
					</tbody>
					{#if report.rows.length}
						{@const t = report.total}
						<tfoot class="border-t border-slate-100 font-medium text-slate-900">
							<tr>
								<td class="px-4 py-2">Total</td><td class="px-3 py-2 text-right">{t.visits}</td><td class="px-3 py-2 text-right">{t.visitors}</td><td class="px-3 py-2 text-right">{t.new_visitors}</td>
								<td class="px-3 py-2 text-right">{t.records}</td><td class="px-3 py-2 text-right">{t.conversion === null ? '—' : `${t.conversion}%`}</td><td class="px-3 py-2 text-right">{t.deals}</td><td class="px-3 py-2 text-right">{t.won}</td>
								<td class="px-3 py-2 text-right">{money(t.revenue, cur)}</td><td class="px-3 py-2 text-right">{Number(t.spend) ? money(t.spend, cur) : '—'}</td><td class="px-3 py-2 text-right">{t.cost_per_record ? money(t.cost_per_record, cur) : '—'}</td><td class="px-4 py-2 text-right">{t.roi === null ? '—' : `${t.roi}%`}</td>
							</tr>
						</tfoot>
					{/if}
				</table>
			</section>
			<p class="text-xs text-slate-400">Records, deals and revenue count for the way each visitor first came (first touch). Deals count when they are the visitor’s record or belong to the visitor’s contact or company. Money is in {cur}.</p>
		{/if}
	{:else if tab === 'sites'}
		<section class="card divide-y divide-slate-50">
			{#each sites as s (s.id)}
				<div class="space-y-2 px-5 py-4 text-sm" data-site={s.name}>
					<div class="flex flex-wrap items-center gap-3">
						<span class="font-medium text-slate-900">{s.name}</span>
						<span class="text-slate-500">{s.domains.length ? s.domains.join(', ') : 'any host'}</span>
						<span class="text-slate-500">{s.visits_30d} visits in 30 days</span>
						<button class="btn btn-ghost ml-auto p-1.5 text-slate-400 hover:text-red-600" aria-label="Remove website" onclick={() => removeSite(s)}><Icon name="trash" class="size-4" /></button>
					</div>
					<div class="flex gap-2">
						<input class="input font-mono text-xs" readonly value={s.snippet} aria-label="Tracking code for {s.name}" onfocus={(e) => e.currentTarget.select()} />
						<button class="btn btn-secondary" onclick={() => copy(s.snippet)}>Copy</button>
					</div>
				</div>
			{:else}
				<p class="px-5 py-6 text-sm text-slate-500">No websites yet.</p>
			{/each}
		</section>
		<form class="card grid gap-3 p-5 sm:grid-cols-3" onsubmit={addSite}>
			<h2 class="font-medium text-slate-900 sm:col-span-3">Add a website</h2>
			<div><label class="label" for="s-name">Name</label><input id="s-name" class="input" required bind:value={newSite.name} /></div>
			<div class="sm:col-span-2"><label class="label" for="s-dom">Host names <span class="font-normal text-slate-400">(optional: only reports from these count)</span></label><input id="s-dom" class="input" placeholder="example.com, shop.example.com" bind:value={newSite.domains} /></div>
			<p class="text-xs text-slate-500 sm:col-span-2">Put the code in every page, before <code>&lt;/body&gt;</code>. NERP web forms and the live chat on those pages then tell the CRM which visitor they were, so records show the visits that led to them. Browsers asking not to be tracked are left alone.</p>
			<div class="flex items-end justify-end"><button class="btn btn-primary">Add website</button></div>
		</form>
	{:else if tab === 'links'}
		<section class="card overflow-x-auto">
			<table class="min-w-full text-sm">
				<thead class="text-left text-xs text-slate-500"><tr><th class="px-4 py-2 font-medium">Ad</th><th class="px-3 py-2 font-medium">Short link</th><th class="px-3 py-2 font-medium">Source</th><th class="px-3 py-2 text-right font-medium">Clicks</th><th class="px-3 py-2 text-right font-medium">Visits</th><th class="px-3 py-2 text-right font-medium">Records</th><th class="px-4 py-2"></th></tr></thead>
				<tbody class="divide-y divide-slate-50 tabular-nums">
					{#each links as l (l.id)}
						<tr class={l.is_active ? '' : 'opacity-60'} data-ad-link={l.name}>
							<td class="px-4 py-2"><span class="text-slate-900">{l.name}</span><span class="ml-1.5 text-xs text-slate-400">{l.kind}</span><div class="max-w-72 truncate text-xs text-slate-500">{l.target_url}</div></td>
							<td class="px-3 py-2"><button class="font-mono text-xs text-brand-700 hover:underline" onclick={() => copy(l.short_url)}>{l.short_url}</button></td>
							<td class="px-3 py-2 text-slate-600">{l.source}</td>
							<td class="px-3 py-2 text-right">{l.clicks}</td><td class="px-3 py-2 text-right">{l.visits}</td><td class="px-3 py-2 text-right">{l.records}</td>
							<td class="px-4 py-2 text-right whitespace-nowrap">
								<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => qr(l)}>QR code</button>
								<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => toggleLink(l)}>{l.is_active ? 'Switch off' : 'Switch on'}</button>
							</td>
						</tr>
					{:else}
						<tr><td colspan="7" class="px-4 py-6 text-center text-slate-500">No ad links yet.</td></tr>
					{/each}
				</tbody>
			</table>
		</section>
		<form class="card grid gap-3 p-5 sm:grid-cols-2" onsubmit={addLink}>
			<h2 class="font-medium text-slate-900 sm:col-span-2">New ad link</h2>
			<div><label class="label" for="l-name">Name</label><input id="l-name" class="input" required placeholder="Spring poster, station" bind:value={newLink.name} /></div>
			<div><label class="label" for="l-url">Page it leads to</label><input id="l-url" class="input" type="url" required placeholder="https://example.com/spring" bind:value={newLink.target_url} /></div>
			<div>
				<label class="label" for="l-src">Source</label>
				<input id="l-src" class="input" required list="l-sources" placeholder="poster, radio, magazine…" bind:value={newLink.source} />
				<datalist id="l-sources">{#each sources as s (s.code)}<option value={s.code}>{s.name}</option>{/each}</datalist>
			</div>
			<div><label class="label" for="l-camp">Campaign <span class="font-normal text-slate-400">(optional)</span></label><input id="l-camp" class="input" bind:value={newLink.campaign} /></div>
			<div class="flex gap-4 text-sm">
				<label class="flex items-center gap-1.5"><input type="radio" value="offline" bind:group={newLink.kind} /> Offline (posters, print, radio, events)</label>
				<label class="flex items-center gap-1.5"><input type="radio" value="online" bind:group={newLink.kind} /> Online</label>
			</div>
			<div class="flex items-end justify-end"><button class="btn btn-primary">Make link</button></div>
		</form>
	{:else}
		<div class="flex items-center gap-2 text-sm">
			<button class="btn btn-ghost p-1.5" aria-label="Previous year" onclick={() => (year--, loadSpend())}><Icon name="chevronRight" class="size-4 rotate-180" /></button>
			<span class="font-medium text-slate-900">{year}</span>
			<button class="btn btn-ghost p-1.5" aria-label="Next year" onclick={() => (year++, loadSpend())}><Icon name="chevronRight" class="size-4" /></button>
		</div>
		<section class="card overflow-x-auto">
			<table class="min-w-full text-sm tabular-nums">
				<thead class="text-left text-xs text-slate-500"><tr><th class="px-4 py-2 font-medium">Channel or source</th><th class="px-3 py-2 font-medium">Month</th><th class="px-3 py-2 text-right font-medium">Amount</th><th class="px-3 py-2 text-right font-medium">In {base}</th><th class="px-3 py-2 font-medium">Note</th><th></th></tr></thead>
				<tbody class="divide-y divide-slate-50">
					{#each spend as s (s.source + s.month)}
						<tr><td class="px-4 py-2">{CHANNEL[s.source] ?? s.source}</td><td class="px-3 py-2">{s.month}</td><td class="px-3 py-2 text-right">{money(s.amount, s.currency)}</td><td class="px-3 py-2 text-right">{money(s.amount_base, base)}</td><td class="px-3 py-2 text-slate-500">{s.note ?? ''}</td>
							<td class="pr-3 text-right"><button class="text-slate-300 hover:text-red-600" aria-label="Remove" onclick={() => removeSpend(s)}><Icon name="x" class="size-4" /></button></td></tr>
					{:else}
						<tr><td colspan="6" class="px-4 py-6 text-center text-slate-500">Nothing entered for {year}.</td></tr>
					{/each}
				</tbody>
			</table>
		</section>
		<form class="card grid gap-3 p-5 sm:grid-cols-5" onsubmit={saveSpend}>
			<div class="sm:col-span-2">
				<label class="label" for="sp-src">Channel or source</label>
				<input id="sp-src" class="input" required list="sp-list" bind:value={newSpend.source} />
				<datalist id="sp-list">{#each Object.entries(CHANNEL) as [k, l] (k)}<option value={k}>{l}</option>{/each}{#each sources as s (s.code)}<option value={s.code}>{s.name}</option>{/each}</datalist>
			</div>
			<div><label class="label" for="sp-m">Month</label><input id="sp-m" class="input" type="month" required bind:value={newSpend.month} /></div>
			<div><label class="label" for="sp-a">Amount</label><input id="sp-a" class="input" type="number" min="0" step="0.01" required bind:value={newSpend.amount} /></div>
			<div><label class="label" for="sp-c">Currency</label><select id="sp-c" class="input" bind:value={newSpend.currency}><option value="">{base}</option>{#each (crm.config?.currencies ?? []).filter((c) => c.code !== base) as c (c.code)}<option>{c.code}</option>{/each}</select></div>
			<div class="sm:col-span-4"><label class="label" for="sp-n">Note</label><input id="sp-n" class="input" bind:value={newSpend.note} /></div>
			<div class="flex items-end justify-end"><button class="btn btn-primary">Save</button></div>
		</form>
	{/if}
</div>
