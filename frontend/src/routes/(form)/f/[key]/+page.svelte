<script lang="ts">
	// A web form for visitors: hosted at /f/{key}, or embedded on another site through
	// /embed.js (then `embed=1`, and the host page's address arrives as `u` / `r`).
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import WebFormView, { type PublicForm } from '#lib/components/crm/WebFormView.svelte';

	const key = $derived(page.params.key);
	const embed = page.url.searchParams.get('embed') === '1';
	// The page the visitor is on, and where they came from.
	const pageUrl = page.url.searchParams.get('u') || (embed ? '' : location.href);
	const referrer = page.url.searchParams.get('r') || (embed ? '' : document.referrer);

	let form = $state<PublicForm | null>(null);
	let missing = $state(false);
	let box: HTMLDivElement | undefined = $state();

	function utm(): Record<string, string> {
		const out: Record<string, string> = {};
		for (const src of [pageUrl, location.href]) {
			try {
				const q = new URL(src).searchParams;
				for (const k of ['source', 'medium', 'campaign', 'content', 'term']) {
					const v = q.get(`utm_${k}`);
					if (v && !out[k]) out[k] = v;
				}
			} catch {
				/* not a URL */
			}
		}
		return out;
	}

	onMount(async () => {
		const res = await fetch(`/api/public/forms/${encodeURIComponent(key!)}`);
		if (!res.ok) {
			missing = true;
			return;
		}
		form = await res.json();
		document.title = form!.settings.title || form!.name;
	});

	// Embedded: blend into the host page.
	$effect(() => {
		if (!embed) return;
		const prev = document.body.style.background;
		document.body.style.background = 'transparent';
		return () => (document.body.style.background = prev);
	});

	// Embedded: tell the host page how tall the form is.
	$effect(() => {
		if (!embed || !box) return;
		const post = () => parent.postMessage({ nerpForm: key, height: box!.scrollHeight + 8 }, '*');
		const ro = new ResizeObserver(post);
		ro.observe(box);
		post();
		return () => ro.disconnect();
	});

	async function submit(values: Record<string, string | boolean>, website: string, consent: boolean): Promise<string | null> {
		const res = await fetch(`/api/public/forms/${encodeURIComponent(key!)}/submit`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ values, website, consent, token: form!.token, utm: utm(), page_url: pageUrl || undefined, referrer: referrer || undefined, visitor_id: page.url.searchParams.get('vid') || undefined })
		});
		const data = await res.json().catch(() => ({}));
		if (!res.ok) throw new Error(data?.error?.message ?? 'Something went wrong. Please try again.');
		if (data.redirect_url) {
			if (embed) parent.postMessage({ nerpForm: key, redirect: data.redirect_url }, '*');
			else location.href = data.redirect_url;
		}
		return data.message;
	}
</script>

<div bind:this={box} class={embed ? 'p-1' : 'flex min-h-screen items-start justify-center bg-slate-100 px-4 py-10 sm:items-center'}>
	<div class="w-full max-w-lg">
		{#if missing}
			<p class="rounded-2xl bg-white p-8 text-center text-slate-600 shadow-sm">This form is not available.</p>
		{:else if form}
			<WebFormView {form} onsubmit={submit} />
			{#if !embed && form.company}<p class="mt-4 text-center text-xs text-slate-400">{form.company}</p>{/if}
		{/if}
	</div>
</div>
