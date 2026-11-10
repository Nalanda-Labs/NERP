<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Avatar from '#lib/components/Avatar.svelte';
	import Icon, { type IconName } from '#lib/components/Icon.svelte';
	import NotificationBell from '#lib/components/NotificationBell.svelte';
	import CheckIn from '#lib/components/CheckIn.svelte';
	import WorkDay from '#lib/components/WorkDay.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { formatClock } from '#lib/tasks.ts';
	import { work } from '#lib/work.svelte.ts';
	import { crm } from '#lib/crm.svelte.ts';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { PERM } from '#lib/types.ts';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();
	let mobileNav = $state(false);
	let menuOpen = $state(false);

	onMount(async () => {
		const status = await api.get<{ needs_setup: boolean; company_name: string | null }>('/setup');
		if (status.needs_setup) return goto('/setup', { replace: true });
		session.companyName = status.company_name ?? 'NERP';
		if (!(await session.restore())) {
			const next = page.url.pathname + page.url.search;
			goto(`/login${next !== '/' ? `?next=${encodeURIComponent(next)}` : ''}`, { replace: true });
		}
	});

	// Being signed out elsewhere (expired refresh token, deactivation) lands here.
	$effect(() => {
		if (session.ready && !session.user) goto('/login', { replace: true });
	});

	$effect(() => {
		if (session.user) {
			work.start();
			crm.loadCounters().catch(() => {});
			crm.load().catch(() => {});
		}
	});
	// Time-off requests waiting for my decision.
	let hrPending = $state(0);
	$effect(() => {
		if (!session.user) return;
		api.get<{ items: unknown[] }>('/hr/absences/pending').then((r) => (hrPending = r.items.length)).catch(() => {});
	});
	// Incoming calls.
	let ringing = $state<{ call_id: string; from: string } | null>(null);
	$effect(() => {
		if (!session.user) return;
		const off = [
			realtime.on('call.ring', (d: { call_id: string; from: string }) => (ringing = d)),
			realtime.on('call.ended', (d: { call_id: string }) => ringing?.call_id === d.call_id && (ringing = null))
		];
		return () => off.forEach((f) => f());
	});
	function answer() {
		const id = ringing!.call_id;
		ringing = null;
		goto(`/call/${id}`);
	}
	function notNow() {
		api.post(`/calls/${ringing!.call_id}/decline`, {}).catch(() => {});
		ringing = null;
	}
	// Business-process approvals waiting for me.
	let approvalsWaiting = $state(0);
	$effect(() => {
		if (!session.user) return;
		const load = () => api.get<{ items: unknown[] }>('/approvals').then((r) => (approvalsWaiting = r.items.length)).catch(() => {});
		load();
		return realtime.on('approvals.changed', load);
	});
	// Unread messenger messages.
	let imUnread = $state(0);
	const loadIm = () => api.get<{ messages: number }>('/im/unread').then((r) => (imUnread = r.messages)).catch(() => {});
	$effect(() => {
		if (!session.user) return;
		loadIm();
		const off = [realtime.on('im.message', loadIm), realtime.on('im.read', loadIm), realtime.on('im.chat', loadIm)];
		return () => off.forEach((f) => f());
	});
	// Website chats waiting for this person.
	let chatCounts = $state({ unread: 0, waiting: 0 });
	const loadChatCounts = () => api.get<{ unread: number; waiting: number }>('/chat/counters').then((c) => (chatCounts = c)).catch(() => {});
	$effect(() => {
		if (!session.user) return;
		loadChatCounts();
		const off = [realtime.on('chat.message', loadChatCounts), realtime.on('chat.changed', loadChatCounts)];
		return () => off.forEach((f) => f());
	});
	const crmBadge = (t: string) => () => {
		const n = crm.counters?.types[t]?.attention ?? 0;
		return n ? { n, alert: (crm.counters?.types[t]?.overdue ?? 0) > 0 } : null;
	};
	let now = $state(Date.now());
	$effect(() => {
		const t = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(t);
	});

	type NavItem = { href: string; label: string; icon: IconName; perm?: string; /** Also shown with this permission. */ orPerm?: string; badge?: () => { n: number; alert: boolean } | null };
	// Smart processes the user may view, after the built-in CRM types.
	const customTypes = $derived(
		(crm.config?.types ?? [])
			.filter((t) => !t.is_system && t.rights.read !== 'none')
			.map((t): NavItem => ({ href: `/crm/${t.code}`, label: t.name_plural, icon: t.icon as IconName, badge: crmBadge(String(t.id)) }))
	);
	const nav: { title?: string; items: NavItem[] }[] = $derived([
		{
			items: [
				{ href: '/', label: 'Dashboard', icon: 'home' },
				{ href: '/feed', label: 'News feed', icon: 'list' },
				{
					href: '/tasks',
					label: 'Tasks',
					icon: 'checkSquare',
					badge: () => (work.counters?.my_open ? { n: work.counters.my_open, alert: work.counters.my_overdue > 0 } : null)
				},
				{ href: '/messenger', label: 'Messenger', icon: 'message', badge: () => (imUnread ? { n: imUnread, alert: false } : null) },
				{ href: '/calendar', label: 'Calendar', icon: 'calendar' },
				{ href: '/booking', label: 'Booking', icon: 'clock' },
				{ href: '/projects', label: 'Workgroups', icon: 'briefcase' },
				{ href: '/employees', label: 'Employees', icon: 'users', perm: PERM.USERS_VIEW },
				{ href: '/structure', label: 'Company structure', icon: 'structure' },
				{ href: '/drive', label: 'Drive', icon: 'folder' },
				{ href: '/kb', label: 'Knowledge base', icon: 'file' },
				{ href: '/lists', label: 'Lists', icon: 'list' },
				{ href: '/approvals', label: 'Approvals', icon: 'check', badge: () => (approvalsWaiting ? { n: approvalsWaiting, alert: true } : null) },
				{ href: '/files', label: 'My uploads', icon: 'paperclip' }
			]
		},
		{
			title: 'CRM',
			items: [
				{ href: '/crm/deal', label: 'Deals', icon: 'funnel', badge: crmBadge('2') },
				{ href: '/crm/lead', label: 'Leads', icon: 'send', badge: crmBadge('1') },
				{ href: '/crm/contact', label: 'Contacts', icon: 'user' },
				{ href: '/crm/company', label: 'Companies', icon: 'building' },
				...customTypes,
				{
					href: '/crm/activities',
					label: 'Activities',
					icon: 'calendar',
					badge: () => (crm.counters?.activities_overdue ? { n: crm.counters.activities_overdue, alert: true } : null)
				},
				{ href: '/crm/analytics', label: 'Analytics', icon: 'columns' },
				{ href: '/crm/products', label: 'Products', icon: 'briefcase' },
				{ href: '/crm/shop', label: 'Online shop', icon: 'store', perm: PERM.SHOP },
				{ href: '/crm/documents', label: 'Documents', icon: 'file' },
				{ href: '/crm/mailboxes', label: 'Mailboxes', icon: 'mail' },
				{ href: '/crm/webforms', label: 'Web forms', icon: 'list', perm: PERM.CRM_ADMIN },
				{ href: '/sites', label: 'Websites', icon: 'globe', perm: PERM.SITES },
				{ href: '/crm/recycle-bin', label: 'Recycle bin', icon: 'trash' }
			]
		},
		{
			title: 'HR',
			items: [
				{ href: '/hr/checkin', label: 'Check-in', icon: 'check' },
				{ href: '/hr/time', label: 'Working time', icon: 'clock' },
				{ href: '/hr/absences', label: 'Time off', icon: 'calendar', badge: () => (hrPending ? { n: hrPending, alert: false } : null) },
				{ href: '/hr/settings', label: 'HR settings', icon: 'settings', perm: PERM.HR }
			]
		},
		{
			title: 'Contact centre',
			items: [
				{ href: '/chats', label: 'Chats', icon: 'message', badge: () => (chatCounts.unread + chatCounts.waiting ? { n: Math.max(chatCounts.unread, chatCounts.waiting), alert: chatCounts.unread > 0 } : null) },
				{ href: '/chats/channels', label: 'Chat buttons', icon: 'globe' }
			]
		},
		{
			title: 'Inventory',
			items: [
				{ href: '/inventory', label: 'Stock', icon: 'columns' },
				{ href: '/inventory/documents', label: 'Stock documents', icon: 'file' },
				{ href: '/inventory/warehouses', label: 'Warehouses', icon: 'building', perm: PERM.INVENTORY, orPerm: PERM.CRM_ADMIN }
			]
		},
		{
			title: 'Accounting',
			items: [
				{ href: '/accounting', label: 'Overview', icon: 'columns', perm: PERM.ACCOUNTING_VIEW, orPerm: PERM.ACCOUNTING_POST },
				{ href: '/accounting/invoices', label: 'Sales invoices', icon: 'file', perm: PERM.ACCOUNTING_VIEW, orPerm: PERM.ACCOUNTING_POST },
				{ href: '/accounting/bills', label: 'Bills', icon: 'paperclip', perm: PERM.ACCOUNTING_VIEW, orPerm: PERM.ACCOUNTING_POST },
				{ href: '/accounting/payments', label: 'Payments', icon: 'send', perm: PERM.ACCOUNTING_VIEW, orPerm: PERM.ACCOUNTING_POST },
				{ href: '/accounting/banking', label: 'Banking', icon: 'building', perm: PERM.ACCOUNTING_VIEW, orPerm: PERM.ACCOUNTING_POST },
				{ href: '/accounting/journal', label: 'Journal', icon: 'list', perm: PERM.ACCOUNTING_VIEW, orPerm: PERM.ACCOUNTING_POST },
				{ href: '/accounting/reports', label: 'Reports', icon: 'gantt', perm: PERM.ACCOUNTING_VIEW, orPerm: PERM.ACCOUNTING_POST },
				{ href: '/accounting/accounts', label: 'Chart of accounts', icon: 'structure', perm: PERM.ACCOUNTING_VIEW, orPerm: PERM.ACCOUNTING_POST },
				{ href: '/accounting/settings', label: 'Accounting settings', icon: 'settings', perm: PERM.ACCOUNTING_ADMIN }
			]
		},
		{
			title: 'Marketing',
			items: [
				{ href: '/marketing/campaigns', label: 'Campaigns', icon: 'send', perm: PERM.MARKETING },
				{ href: '/marketing/segments', label: 'Segments', icon: 'users', perm: PERM.MARKETING },
				{ href: '/marketing/optouts', label: 'Unsubscribes', icon: 'mail', perm: PERM.MARKETING }
			]
		},
		{
			title: 'Administration',
			items: [
				{ href: '/crm/settings', label: 'CRM settings', icon: 'funnel', perm: PERM.CRM_ADMIN },
				{ href: '/roles', label: 'Roles & permissions', icon: 'shield', perm: PERM.ROLES_MANAGE },
				{ href: '/audit', label: 'Audit log', icon: 'history', perm: PERM.AUDIT_VIEW },
				{ href: '/developers', label: 'API & webhooks', icon: 'key', perm: PERM.API_KEYS, orPerm: PERM.WEBHOOKS_MANAGE },
				{ href: '/system', label: 'System', icon: 'cpu', perm: PERM.SYSTEM_MANAGE },
				{ href: '/settings/directory', label: 'Directory & sign-on', icon: 'lock', perm: PERM.SETTINGS_MANAGE },
				{ href: '/settings', label: 'Settings', icon: 'settings', perm: PERM.SETTINGS_MANAGE }
			]
		}
	]);

	// Collapsed sidebar groups, remembered on this device.
	const COLLAPSED_KEY = 'nav.collapsed';
	let collapsed = $state<Record<string, boolean>>(
		(() => {
			try {
				return JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? '{}');
			} catch {
				return {};
			}
		})()
	);
	function toggleGroup(title: string) {
		collapsed[title] = !collapsed[title];
		try {
			localStorage.setItem(COLLAPSED_KEY, JSON.stringify(collapsed));
		} catch {
			/* private mode: just not remembered */
		}
	}
	/** A collapsed group's badges, added up (alert if any of them is). */
	function groupBadge(items: NavItem[]) {
		const badges = items.map((it) => it.badge?.()).filter((b) => !!b);
		return badges.length ? { n: badges.reduce((a, b) => a + b.n, 0), alert: badges.some((b) => b.alert) } : null;
	}

	const under = (href: string) => (href === '/' ? page.url.pathname === '/' : page.url.pathname === href || page.url.pathname.startsWith(href + '/'));
	/** The closest item: "/inventory/documents/…" lights "Stock documents", not also "Stock". */
	const isActive = (href: string) => under(href) && !nav.some((g) => g.items.some((it) => it.href.length > href.length && it.href.startsWith(href) && under(it.href)));

	async function logout() {
		await session.logout();
		goto('/login', { replace: true });
	}
</script>

{#if session.user}
	{@const user = session.user}
	<div class="flex min-h-screen">
		<!-- Sidebar -->
		<aside
			class="fixed inset-y-0 left-0 z-40 flex w-64 flex-col border-r border-slate-200 bg-surface text-slate-700 transition-transform lg:translate-x-0
				{mobileNav ? 'translate-x-0' : '-translate-x-full'}"
		>
			<div class="flex h-16 items-center gap-2.5 px-5">
				<span class="flex size-9 items-center justify-center rounded-full bg-brand-600 text-lg font-bold text-white">N</span>
				<span class="truncate text-lg text-slate-700">{session.companyName}</span>
			</div>
			<nav class="flex-1 space-y-5 overflow-y-auto py-2 pr-3">
				{#each nav as section, i (i)}
					{@const items = section.items.filter((it) => !it.perm || session.can(it.perm) || (it.orPerm && session.can(it.orPerm)))}
					{#if items.length}
						{@const closed = !!section.title && !!collapsed[section.title]}
						<div>
							{#if section.title}
								{@const gb = closed ? groupBadge(items) : null}
								<button
									class="mb-1 flex w-full items-center gap-2 rounded-r-full py-1 pr-4 pl-6 text-left text-sm font-medium text-slate-500 hover:bg-surface-hover hover:text-slate-700"
									aria-expanded={!closed}
									aria-controls="nav-{i}"
									onclick={() => toggleGroup(section.title!)}
								>
									<span class="flex-1">{section.title}</span>
									{#if gb}<span class="text-xs font-semibold {gb.alert ? 'rounded-full bg-red-600 px-1.5 text-white' : 'text-slate-600'}">{gb.n}</span>{/if}
									<Icon name="chevronDown" class="size-4 transition-transform {closed ? '-rotate-90' : ''}" />
								</button>
							{/if}
							<ul id="nav-{i}" class="space-y-0.5">
								<!-- A collapsed group still shows the page you are on. -->
								{#each items.filter((it) => !closed || isActive(it.href)) as item (item.href)}
									<li>
										<a
											href={item.href}
											onclick={() => (mobileNav = false)}
											class="flex h-9 items-center gap-4 rounded-r-full pr-4 pl-6 text-sm transition
												{isActive(item.href) ? 'bg-brand-100 font-semibold text-brand-900' : 'text-slate-700 hover:bg-surface-hover'}"
										>
											<Icon name={item.icon} class="size-5 shrink-0" />
											{item.label}
											{#if item.badge?.()}
												{@const b = item.badge()!}
												<span class="ml-auto text-xs font-semibold {b.alert ? 'rounded-full bg-red-600 px-1.5 text-white' : 'text-slate-600'}">{b.n}</span>
											{/if}
										</a>
									</li>
								{/each}
							</ul>
						</div>
					{/if}
				{/each}
			</nav>
			<div class="px-6 py-3 text-xs text-slate-400">NERP</div>
		</aside>
		{#if mobileNav}
			<button class="fixed inset-0 z-30 bg-slate-900/40 lg:hidden" aria-label="Close menu" onclick={() => (mobileNav = false)}></button>
		{/if}

		<!-- Main -->
		<div class="flex min-w-0 flex-1 flex-col lg:pl-64">
			<header class="sticky top-0 z-20 flex h-16 items-center gap-3 bg-surface/95 px-4 backdrop-blur sm:px-6">
				<button class="btn btn-ghost p-2 lg:hidden" aria-label="Open menu" onclick={() => (mobileNav = true)}>
					<Icon name="menu" />
				</button>
				<div class="flex-1"></div>
				{#if work.timer}
					<div class="flex items-center gap-1 rounded-full border border-red-200 bg-red-50 py-1 pr-1 pl-3 text-sm text-red-700">
						<a href="/tasks/{work.timer.task_id}" class="flex max-w-56 items-center gap-2 truncate" title={work.timer.title}>
							<span class="size-2 animate-pulse rounded-full bg-red-500"></span>
							<span class="font-mono">{formatClock((now - new Date(work.timer.started_at).getTime()) / 1000)}</span>
							<span class="hidden truncate md:inline">#{work.timer.number} {work.timer.title}</span>
						</a>
						<button class="rounded-full p-1 hover:bg-red-100" aria-label="Stop timer" onclick={() => work.stopTimer()}><Icon name="stop" class="size-3.5" /></button>
					</div>
				{/if}
				<CheckIn />
				<WorkDay />
				<NotificationBell />
				<div class="relative">
					<button
						class="flex items-center gap-2.5 rounded-lg p-1.5 pr-2 hover:bg-slate-100"
						onclick={() => (menuOpen = !menuOpen)}
						onblur={() => setTimeout(() => (menuOpen = false), 150)}
					>
						<Avatar name={user.full_name} fileId={user.avatar_file_id} size="sm" />
						<span class="hidden text-left sm:block">
							<span class="block text-sm font-medium text-slate-800">{user.full_name}</span>
							<span class="block text-xs text-slate-500">{user.position ?? user.email}</span>
						</span>
						<Icon name="chevronDown" class="size-4 text-slate-400" />
					</button>
					{#if menuOpen}
						<div class="absolute right-0 mt-1 w-52 rounded-xl bg-white py-1 shadow-g2">
							<a href="/profile" class="flex items-center gap-2 px-3 py-2 text-sm text-slate-700 hover:bg-slate-50">
								<Icon name="user" class="size-4" /> My profile
							</a>
							<button onmousedown={logout} class="flex w-full items-center gap-2 px-3 py-2 text-sm text-slate-700 hover:bg-slate-50">
								<Icon name="logout" class="size-4" /> Sign out
							</button>
						</div>
					{/if}
				</div>
			</header>
			<main class="mx-auto w-full max-w-7xl flex-1 px-4 py-6 sm:px-6 lg:px-8">
				{@render children()}
			</main>
		</div>
	</div>
{:else}
	<div class="flex min-h-screen items-center justify-center text-sm text-slate-400">Loading…</div>
{/if}

{#if ringing}
	<div class="fixed right-6 bottom-6 z-50 w-80 rounded-2xl bg-slate-900 p-4 text-white shadow-g2" role="alertdialog" aria-label="Incoming call" data-ringing>
		<p class="font-medium">{ringing.from} is calling</p>
		<div class="mt-3 flex gap-2">
			<button class="btn flex-1 justify-center bg-emerald-600 text-white hover:bg-emerald-700" onclick={answer}>Join</button>
			<button class="btn flex-1 justify-center bg-slate-700 text-white hover:bg-slate-600" onclick={notNow}>Not now</button>
		</div>
	</div>
{/if}
