<script lang="ts">
	// Public: the client portal. Signed in (a cookie from the email link): the client's
	// invoices, quotes, payments, signatures, tickets and bookings. Otherwise: ask for a link.
	import { onMount } from 'svelte';

	type Doc = { id: string; number: number; title: string; currency: string; amount: string; date: string; due_date: string | null; status: string; balance: string | null; pdf: string | null };
	type Me = {
		client: { name: string; company: string | null; sees_company: boolean };
		company: string;
		invoices: Doc[];
		quotes: Doc[];
		payments: { number: number; date: string; amount: string; currency: string; reference: string | null }[];
		signatures: { id: string; title: string; status: string; sent_at: string; completed_at: string | null; signed_copy: string | null }[];
		tickets: { number: number; title: string; open: boolean; created_at: string; last_message_at: string | null; link: string | null }[];
		bookings: { number: number; starts_at: string; ends_at: string; status: string; service: string | null }[];
	};
	let me = $state<Me | null>(null);
	let signedOut = $state(false);
	let email = $state('');
	let sent = $state(false);
	let error = $state('');

	onMount(async () => {
		const r = await fetch('/api/public/portal/me', { credentials: 'same-origin' });
		if (r.ok) me = await r.json();
		else signedOut = true;
	});
	async function ask(e: SubmitEvent) {
		e.preventDefault();
		error = '';
		const r = await fetch('/api/public/portal/request', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email }) });
		if (r.ok) sent = true;
		else error = r.status === 429 ? 'Too many requests. Please try again later.' : 'Please check the address.';
	}
	async function signOut() {
		await fetch('/api/public/portal/sign-out', { method: 'POST' });
		me = null;
		signedOut = true;
	}
	const day = (d: string | null) => (d ? new Date(d).toLocaleDateString() : '—');
	const when = (d: string) => new Date(d).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
	const money = (v: string | null, c: string) => (v === null ? '—' : `${Number(v).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })} ${c}`);
	const STATUS: Record<string, string> = { open: 'To pay', paid: 'Paid', credited: 'Settled', issued: 'Issued' };
</script>

<svelte:head><title>Client portal</title><meta name="robots" content="noindex" /></svelte:head>

<main class="min-h-screen bg-slate-50 p-4 sm:p-8">
	{#if me}
		<div class="mx-auto max-w-4xl space-y-5">
			<header class="flex flex-wrap items-center gap-3">
				<div>
					<p class="text-sm text-slate-500">{me.company}</p>
					<h1 class="text-2xl font-semibold text-slate-900" data-portal-name>{me.client.name}{me.client.company ? ` · ${me.client.company}` : ''}</h1>
				</div>
				<button class="btn btn-ghost ml-auto" onclick={signOut}>Sign out</button>
			</header>

			{#snippet docs(title: string, list: Doc[], invoices: boolean)}
				<section class="rounded-2xl bg-white p-5 shadow-g1" aria-label={title}>
					<h2 class="font-semibold text-slate-900">{title}</h2>
					<table class="mt-2 min-w-full text-sm">
						<tbody class="divide-y divide-slate-100">
							{#each list as d (d.id)}
								<tr data-doc={d.number}>
									<td class="py-2">#{d.number} <span class="text-slate-600">{d.title}</span></td>
									<td class="text-slate-500">{day(d.date)}</td>
									<td class="text-right tabular-nums">{money(d.amount, d.currency)}</td>
									{#if invoices}<td class="text-right"><span class="rounded-full px-2 py-0.5 text-xs {d.status === 'open' ? 'bg-amber-50 text-amber-800' : 'bg-emerald-50 text-emerald-800'}">{STATUS[d.status] ?? d.status}</span>{#if d.status === 'open' && d.balance}<span class="ml-1 text-xs text-slate-500">{money(d.balance, d.currency)} left{d.due_date ? `, due ${day(d.due_date)}` : ''}</span>{/if}</td>{/if}
									<td class="text-right">{#if d.pdf}<a class="text-brand-700 hover:underline" href={d.pdf}>PDF</a>{/if}</td>
								</tr>
							{:else}<tr><td class="py-2 text-slate-500">None.</td></tr>{/each}
						</tbody>
					</table>
				</section>
			{/snippet}
			{@render docs('Invoices', me.invoices, true)}
			{#if me.quotes.length}{@render docs('Quotes', me.quotes, false)}{/if}

			<section class="rounded-2xl bg-white p-5 shadow-g1" aria-label="Payments">
				<h2 class="font-semibold text-slate-900">Payments received</h2>
				<ul class="mt-2 space-y-1 text-sm">
					{#each me.payments as p (p.number)}<li>{day(p.date)} · {money(p.amount, p.currency)}{p.reference ? ` · ${p.reference}` : ''}</li>{:else}<li class="text-slate-500">None.</li>{/each}
				</ul>
			</section>

			{#if me.signatures.length}
				<section class="rounded-2xl bg-white p-5 shadow-g1" aria-label="Signatures">
					<h2 class="font-semibold text-slate-900">Documents to sign and signed</h2>
					<ul class="mt-2 space-y-1 text-sm">
						{#each me.signatures as s (s.id)}<li>{s.title} · {s.status === 'completed' ? `signed ${day(s.completed_at)}` : s.status}{#if s.signed_copy} · <a class="text-brand-700 hover:underline" href={s.signed_copy}>Signed copy</a>{/if}</li>{/each}
					</ul>
				</section>
			{/if}

			<section class="rounded-2xl bg-white p-5 shadow-g1" aria-label="Tickets">
				<h2 class="font-semibold text-slate-900">Your requests</h2>
				<ul class="mt-2 space-y-1 text-sm">
					{#each me.tickets as t (t.number)}
						<li data-ticket={t.number}>{#if t.link}<a class="text-brand-700 hover:underline" href={t.link}>#{t.number} {t.title}</a>{:else}#{t.number} {t.title}{/if} <span class="text-xs text-slate-500">· {t.open ? 'open' : 'closed'} · {day(t.created_at)}</span></li>
					{:else}<li class="text-slate-500">None. <a class="text-brand-700 hover:underline" href="/help">Ask for help</a></li>{/each}
				</ul>
			</section>

			{#if me.bookings.length}
				<section class="rounded-2xl bg-white p-5 shadow-g1" aria-label="Bookings">
					<h2 class="font-semibold text-slate-900">Bookings</h2>
					<ul class="mt-2 space-y-1 text-sm">
						{#each me.bookings as b (b.number)}<li>{when(b.starts_at)} · {b.service ?? 'Appointment'} <span class="text-xs text-slate-500">· {b.status}</span></li>{/each}
					</ul>
				</section>
			{/if}
		</div>
	{:else if signedOut}
		<div class="mx-auto mt-16 max-w-md rounded-3xl bg-white p-8 shadow-g1">
			{#if sent}
				<h1 class="text-xl font-semibold text-slate-900" data-link-sent>Check your email</h1>
				<p class="mt-2 text-sm text-slate-600">If {email} has access to the portal, a sign-in link is on its way. It works once, within 7 days.</p>
			{:else}
				<h1 class="text-xl font-semibold text-slate-900">Client portal</h1>
				<p class="mt-1 text-sm text-slate-600">Your invoices, payments, documents, requests and bookings in one place. Enter your email for a sign-in link.</p>
				<form class="mt-4 space-y-3 text-sm" onsubmit={ask}>
					<input class="input" type="email" required placeholder="you@example.com" bind:value={email} aria-label="Email" />
					{#if error}<p class="text-red-700">{error}</p>{/if}
					<button class="btn btn-primary w-full">Send me a link</button>
				</form>
			{/if}
		</div>
	{:else}
		<p class="mt-16 text-center text-sm text-slate-400">Loading…</p>
	{/if}
</main>
