<script lang="ts">
	// One website: its pages, look, publishing and visits.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, downloadUrl, requestBlob } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type P = {
		id: string;
		path: string;
		title: string;
		in_menu: boolean;
		published_at: string | null;
		changed: boolean;
	};
	type Settings = {
		color: string;
		font: string;
		logo_file_id: string | null;
		footer: string | null;
		analytics_id?: string | null;
		cookie_banner?: boolean;
		cookie_text?: string | null;
	};
	type Site = {
		id: string;
		name: string;
		slug: string;
		url: string;
		domain: string | null;
		settings: Settings;
		page_list: P[];
	};
	let s = $state<Site | null>(null);
	let stats = $state<{ title: string; path: string; week: number; month: number }[]>([]);
	let form = $state({
		name: '',
		slug: '',
		color: '#1a73e8',
		font: 'sans',
		footer: '',
		logo_file_id: null as string | null,
		domain: '',
		analytics_id: '',
		cookie_banner: false,
		cookie_text: ''
	});
	let logo = $state<string | null>(null);
	function take(x: Site) {
		s = x;
		form = {
			name: x.name,
			slug: x.slug,
			color: x.settings.color ?? '#1a73e8',
			font: x.settings.font ?? 'sans',
			footer: x.settings.footer ?? '',
			logo_file_id: x.settings.logo_file_id,
			domain: x.domain ?? '',
			analytics_id: x.settings.analytics_id ?? '',
			cookie_banner: !!x.settings.cookie_banner,
			cookie_text: x.settings.cookie_text ?? ''
		};
		if (form.logo_file_id)
			requestBlob('GET', `/files/${form.logo_file_id}/content`)
				.then((b) => (logo = URL.createObjectURL(b)))
				.catch(() => {});
	}
	$effect(() => {
		api
			.get<Site>(`/sites/${page.params.id}`)
			.then(take)
			.catch((e) => toasts.error(e));
		api
			.get<{ items: typeof stats }>(`/sites/${page.params.id}/stats`)
			.then((r) => (stats = r.items))
			.catch(() => {});
	});
	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			take(
				await api.patch<Site>(`/sites/${s!.id}`, {
					name: form.name,
					slug: form.slug,
					domain: form.domain.trim() || null,
					settings: {
						color: form.color,
						font: form.font,
						footer: form.footer || null,
						logo_file_id: form.logo_file_id,
						analytics_id: form.analytics_id.trim() || null,
						cookie_banner: form.cookie_banner,
						cookie_text: form.cookie_text || null
					}
				})
			);
			toasts.success('Saved');
		} catch (err) {
			toasts.error(err);
		}
	}
	async function uploadLogo(e: Event) {
		const f = (e.currentTarget as HTMLInputElement).files?.[0];
		if (!f) return;
		const fd = new FormData();
		fd.append('file', f);
		const up = await api.post<{ id: string }>('/files', fd);
		form.logo_file_id = up.id;
		logo = URL.createObjectURL(f);
	}
	async function newPage() {
		const title = prompt('Title of the new page');
		if (!title?.trim()) return;
		try {
			const p = await api.post<{ id: string }>(`/sites/${s!.id}/pages`, {
				title,
				blocks: [{ kind: 'text', markdown: `## ${title}\n\nWrite here.` }]
			});
			goto(`/sites/page/${p.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function publishAll() {
		take(await api.post<Site>(`/sites/${s!.id}/publish`, {}));
		toasts.success('All pages published');
	}
	async function copySite() {
		try {
			const c = await api.post<{ id: string }>(`/sites/${s!.id}/copy`, {});
			goto(`/sites/${c.id}`);
		} catch (e) {
			toasts.error(e);
		}
	}
	async function remove() {
		if (!confirm(`Delete the website “${s!.name}”? Its address stops working.`)) return;
		await api.del(`/sites/${s!.id}`);
		goto('/sites');
	}
</script>

<svelte:head><title>{s?.name ?? 'Website'} · NERP</title></svelte:head>

<a href="/sites" class="mb-4 inline-block text-sm text-slate-500 hover:text-slate-800">← Websites</a
>
{#if s}
	<div class="max-w-5xl space-y-5">
		<div class="flex flex-wrap items-center gap-3">
			<div class="min-w-0 flex-1">
				<h1 class="text-2xl font-semibold text-slate-900">{s.name}</h1>
				<a
					class="text-sm text-brand-600 hover:underline"
					href={s.url}
					target="_blank"
					rel="noopener">{s.url}</a
				>
			</div>
			<button class="btn btn-secondary" onclick={newPage}
				><Icon name="plus" class="size-4" /> New page</button
			>
			<button class="btn btn-primary" onclick={publishAll}>Publish all</button>
			<button class="btn btn-ghost" onclick={copySite}>Copy</button>
			<button
				class="btn btn-ghost"
				onclick={() =>
					downloadUrl(`/api/sites/${s!.id}/export`, `${s!.slug}.nerp-site.json`).catch(
						toasts.error
					)}>Export</button
			>
		</div>
		<section class="card divide-y divide-slate-50" aria-label="Pages">
			{#each s.page_list as p (p.id)}
				<a
					href="/sites/page/{p.id}"
					class="flex items-center gap-3 px-5 py-3 hover:bg-slate-50"
					data-page={p.title}
				>
					<span class="flex-1"
						><span class="font-medium text-slate-900">{p.title}</span>
						<span class="text-sm text-slate-400">/{p.path}</span></span
					>
					{#if !p.in_menu}<span class="text-xs text-slate-400">not in the menu</span>{/if}
					{#if !p.published_at}<span class="badge bg-slate-100 text-slate-600">Draft</span
						>{:else if p.changed}<span class="badge bg-amber-50 text-amber-800"
							>Changes not published</span
						>{:else}<span class="badge bg-emerald-50 text-emerald-800"
							>Published {formatRelative(p.published_at)}</span
						>{/if}
				</a>
			{/each}
		</section>
		<div class="grid gap-5 lg:grid-cols-2">
			<form class="card space-y-3 p-5" onsubmit={save} aria-label="Look">
				<h2 class="font-medium text-slate-900">Name, address and look</h2>
				<div class="grid gap-3 sm:grid-cols-2">
					<div>
						<label class="label" for="sx-n">Name</label><input
							id="sx-n"
							class="input"
							required
							bind:value={form.name}
						/>
					</div>
					<div>
						<label class="label" for="sx-s">Address</label><input
							id="sx-s"
							class="input"
							required
							bind:value={form.slug}
						/>
					</div>
					<div>
						<label class="label" for="sx-c">Colour</label><input
							id="sx-c"
							class="input h-10 p-1"
							type="color"
							bind:value={form.color}
						/>
					</div>
					<div>
						<label class="label" for="sx-f">Font</label><select
							id="sx-f"
							class="input"
							bind:value={form.font}
							><option value="sans">Modern (sans-serif)</option><option value="serif"
								>Classic (serif)</option
							></select
						>
					</div>
				</div>
				<div class="flex items-center gap-3">
					<span class="label mb-0">Logo</span>{#if logo}<img
							src={logo}
							alt=""
							class="h-8"
						/>{/if}<label class="btn btn-secondary cursor-pointer px-3 py-1 text-sm"
						>Upload<input
							type="file"
							accept="image/png,image/jpeg,image/webp,image/gif"
							class="sr-only"
							onchange={uploadLogo}
						/></label
					>{#if form.logo_file_id}<button
							type="button"
							class="text-sm text-slate-500 hover:text-red-600"
							onclick={() => ((form.logo_file_id = null), (logo = null))}>Remove</button
						>{/if}
				</div>
				<div>
					<label class="label" for="sx-ft">Footer</label><input
						id="sx-ft"
						class="input"
						maxlength="500"
						placeholder="© Acme Ltd"
						bind:value={form.footer}
					/>
				</div>
				<div class="border-t border-slate-100 pt-3">
					<label class="label" for="sx-dom">Own domain</label>
					<input id="sx-dom" class="input" placeholder="www.example.com" bind:value={form.domain} />
					<p class="mt-1 text-xs text-slate-500">
						Point the domain at the NERP server (a DNS A or CNAME record) and add it to the web
						server in front of NERP with a certificate; the site then opens at that address.
					</p>
				</div>
				<div>
					<label class="label" for="sx-ga">Google Analytics ID</label><input
						id="sx-ga"
						class="input"
						placeholder="G-XXXXXXXXXX"
						bind:value={form.analytics_id}
					/>
					<p class="mt-1 text-xs text-slate-500">
						Works on the site's own domain (never where NERP itself runs).
					</p>
				</div>
				<label class="flex items-center gap-2 text-sm"
					><input
						type="checkbox"
						class="rounded border-slate-300"
						bind:checked={form.cookie_banner}
						data-cookie-toggle
					/> Ask visitors about cookies (Analytics waits for a yes)</label
				>
				{#if form.cookie_banner}<input
						class="input"
						maxlength="500"
						aria-label="Cookie banner text"
						placeholder="We use cookies to see how the site is used and to improve it."
						bind:value={form.cookie_text}
					/>{/if}
				<div class="flex justify-between">
					<button type="button" class="btn btn-ghost hover:text-red-600" onclick={remove}
						>Delete website</button
					><button class="btn btn-primary">Save</button>
				</div>
			</form>
			<section class="card p-5" aria-label="Visits">
				<h2 class="mb-3 font-medium text-slate-900">Visits</h2>
				<table class="w-full text-sm">
					<thead class="text-left text-xs text-slate-500"
						><tr
							><th class="py-1">Page</th><th class="text-right">7 days</th><th class="text-right"
								>30 days</th
							></tr
						></thead
					>
					<tbody
						>{#each stats as v (v.path)}<tr
								><td class="py-1">{v.title}</td><td class="text-right tabular-nums">{v.week}</td><td
									class="text-right tabular-nums">{v.month}</td
								></tr
							>{/each}</tbody
					>
				</table>
			</section>
		</div>
	</div>
{/if}
