<script lang="ts">
	// The public help page: customers send a request and are taken to its private page.
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	type Choice = { id: string; name: string; is_default: boolean };
	type Portal = { company: string; intro: string | null; categories: Choice[]; criticalities: Choice[] };
	let portal = $state<Portal | null>(null);
	let missing = $state(false);
	let f = $state({ name: '', email: '', title: '', body: '', category_id: '', criticality_id: '', website: '' });
	let busy = $state(false);
	let error = $state('');
	onMount(async () => {
		const r = await fetch('/api/public/support');
		if (!r.ok) return void (missing = true);
		portal = await r.json();
		f.category_id = portal!.categories.find((c) => c.is_default)?.id ?? portal!.categories[0]?.id ?? '';
		f.criticality_id = portal!.criticalities.find((c) => c.is_default)?.id ?? '';
	});
	async function send(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			const r = await fetch('/api/public/support/tickets', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ ...f, category_id: f.category_id || null, criticality_id: f.criticality_id || null }) });
			const data = await r.json();
			if (!r.ok) error = data?.error?.message ?? 'Something went wrong; please try again.';
			else await goto(`/help/t/${data.token}?new=${data.number}`);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head><title>{portal ? `Help · ${portal.company}` : 'Help'}</title></svelte:head>

<main class="min-h-dvh bg-slate-50 px-4 py-10">
	<div class="mx-auto max-w-xl rounded-2xl bg-white p-8 shadow-sm">
		{#if missing}
			<p class="text-center text-slate-600">This page isn't available.</p>
		{:else if portal}
			<h1 class="text-2xl font-semibold text-slate-900">How can we help?</h1>
			<p class="mt-1 text-slate-600">{portal.intro ?? `Send your question to ${portal.company}. You'll get an email with a link to follow your request.`}</p>
			<form class="mt-6 space-y-4" onsubmit={send}>
				<div class="grid gap-3 sm:grid-cols-2">
					<div><label class="label" for="h-name">Your name</label><input id="h-name" class="input" required maxlength="200" autocomplete="name" bind:value={f.name} /></div>
					<div><label class="label" for="h-email">Email</label><input id="h-email" class="input" type="email" required autocomplete="email" bind:value={f.email} /></div>
				</div>
				<div><label class="label" for="h-title">Subject</label><input id="h-title" class="input" required maxlength="300" bind:value={f.title} /></div>
				<div class="grid gap-3 sm:grid-cols-2">
					{#if portal.categories.length}
						<div><label class="label" for="h-cat">Topic</label><select id="h-cat" class="input" bind:value={f.category_id}>{#each portal.categories as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</select></div>
					{/if}
					{#if portal.criticalities.length}
						<div><label class="label" for="h-crit">How urgent?</label><select id="h-crit" class="input" bind:value={f.criticality_id}>{#each portal.criticalities as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</select></div>
					{/if}
				</div>
				<div><label class="label" for="h-body">Your message</label><textarea id="h-body" class="input min-h-36" required bind:value={f.body}></textarea></div>
				<input class="hidden" tabindex="-1" autocomplete="off" aria-hidden="true" bind:value={f.website} />
				{#if error}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700" role="alert">{error}</p>{/if}
				<button class="btn btn-primary w-full justify-center" type="submit" disabled={busy}>Send</button>
			</form>
		{/if}
	</div>
</main>
