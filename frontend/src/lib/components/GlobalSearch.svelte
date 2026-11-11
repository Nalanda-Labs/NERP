<script lang="ts">
	// One search box for everything: people, tasks, CRM records, drive files, knowledge base
	// pages, boards and feed posts. Each part asks that module's own search, so everyone
	// finds only what they may see.
	import { goto } from '$app/navigation';
	import { api, query } from '#lib/api.ts';

	type Hit = { group: string; title: string; sub?: string; href: string };
	let q = $state('');
	let hits = $state<Hit[]>([]);
	let open = $state(false);
	let active = $state(0);
	let timer: ReturnType<typeof setTimeout>;
	let seq = 0;
	const CRM: [string, string][] = [['lead', 'Leads'], ['deal', 'Deals'], ['contact', 'Contacts'], ['company', 'Companies']];

	async function run(term: string) {
		const mine = ++seq;
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		const get = (p: string) => api.get<{ items: any[] }>(p).then((r) => r.items ?? []).catch(() => []);
		const parts = await Promise.all([
			get('/users' + query({ q: term, limit: 5 })).then((xs) => xs.map((u): Hit => ({ group: 'People', title: u.full_name, sub: u.position ?? '', href: `/employees/${u.id}` }))),
			get('/tasks' + query({ q: term, limit: 5 })).then((xs) => xs.map((t): Hit => ({ group: 'Tasks', title: `#${t.number} ${t.title}`, href: `/tasks/${t.id}` }))),
			...(term.length >= 3 ? CRM.map(([code, label]) => get(`/crm/${code}/items` + query({ q: term, limit: 4 })).then((xs) => xs.map((i): Hit => ({ group: label, title: i.title, href: `/crm/${code}/${i.id}` })))) : []),
			get('/drive/search' + query({ q: term })).then((xs) => xs.slice(0, 5).map((n): Hit => ({ group: 'Drive', title: n.name, href: n.kind === 'folder' ? `/drive?folder=${n.id}` : `/drive?file=${n.id}${n.parent_id ? `&folder=${n.parent_id}` : `&space=${n.space}`}` }))),
			get('/kb/search' + query({ q: term })).then((xs) => xs.slice(0, 5).map((p): Hit => ({ group: 'Knowledge base', title: p.title, sub: p.space_name ?? p.space?.name ?? '', href: `/kb/page/${p.id}` }))),
			get('/boards' + query({ q: term })).then((xs) => xs.slice(0, 4).map((b): Hit => ({ group: 'Boards', title: b.title, href: `/boards/${b.id}` }))),
			get('/feed' + query({ q: term, limit: 4 })).then((xs) => xs.map((p): Hit => ({ group: 'News feed', title: p.body.slice(0, 80), sub: p.author_name ?? '', href: `/feed?post=${p.id}` })))
		]);
		if (mine === seq) {
			hits = parts.flat();
			active = 0;
		}
	}
	function typed() {
		clearTimeout(timer);
		const term = q.trim();
		if (term.length < 2) {
			hits = [];
			return;
		}
		open = true;
		timer = setTimeout(() => run(term), 250);
	}
	function pick(h: Hit) {
		open = false;
		q = '';
		hits = [];
		goto(h.href);
	}
	function key(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') { active = Math.min(active + 1, hits.length - 1); e.preventDefault(); }
		else if (e.key === 'ArrowUp') { active = Math.max(active - 1, 0); e.preventDefault(); }
		else if (e.key === 'Enter' && hits[active]) { pick(hits[active]); e.preventDefault(); }
		else if (e.key === 'Escape') open = false;
	}
	const groups = $derived([...new Set(hits.map((h) => h.group))]);
</script>

<div class="relative w-72 max-w-full">
	<input class="input py-1.5" type="search" placeholder="Search everything…" aria-label="Search everything" bind:value={q} oninput={typed} onkeydown={key} onfocus={() => hits.length && (open = true)} onblur={() => setTimeout(() => (open = false), 150)} />
	{#if open && q.trim().length >= 2}
		<div class="absolute top-10 left-0 z-50 max-h-[70vh] w-[28rem] max-w-[90vw] overflow-y-auto rounded-xl border border-slate-200 bg-white py-2 shadow-xl" role="listbox" data-search-results>
			{#each groups as g (g)}
				<p class="px-4 pt-2 pb-1 text-xs font-medium tracking-wide text-slate-400 uppercase">{g}</p>
				{#each hits.filter((h) => h.group === g) as h (h.href)}
					<button class="block w-full px-4 py-1.5 text-left text-sm hover:bg-slate-50 {hits[active] === h ? 'bg-slate-100' : ''}" role="option" aria-selected={hits[active] === h} onmousedown={() => pick(h)}>
						<span class="text-slate-800">{h.title}</span>{#if h.sub}<span class="ml-2 text-xs text-slate-400">{h.sub}</span>{/if}
					</button>
				{/each}
			{:else}<p class="px-4 py-2 text-sm text-slate-400">Nothing found.</p>{/each}
		</div>
	{/if}
</div>
