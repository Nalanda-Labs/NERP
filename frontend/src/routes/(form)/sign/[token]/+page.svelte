<script lang="ts">
	// Public: a signer's personal link. Read the PDF, then sign (typed name + drawn
	// signature + consent) or decline.
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';

	type Info = {
		title: string;
		message: string | null;
		company: string;
		name: string;
		email: string;
		status: 'viewed' | 'signed' | 'declined';
		request_status: 'sent' | 'completed' | 'declined' | 'cancelled' | 'expired';
		fingerprint: string;
		expires_at: string;
		signed_available: boolean;
		others: { name: string; status: string }[];
	};
	const token = $derived(page.params.token ?? '');
	let info = $state<Info | null>(null);
	let missing = $state(false);
	let typed = $state('');
	let agree = $state(false);
	let busy = $state(false);
	let error = $state('');
	let declining = $state(false);
	let reason = $state('');

	$effect(() => {
		api.get<Info>(`/public/sign/${encodeURIComponent(token)}`)
			.then((i) => ((info = i), (typed = i.name)))
			.catch(() => (missing = true));
	});
	const docUrl = $derived(`/api/public/sign/${encodeURIComponent(token)}/document`);

	// The signature pad.
	let canvas: HTMLCanvasElement | undefined = $state();
	let drawn = $state(false);
	let drawing = false;
	function ctx() {
		const c = canvas!.getContext('2d')!;
		c.lineWidth = 2.5;
		c.lineCap = 'round';
		c.lineJoin = 'round';
		c.strokeStyle = '#0f172a';
		return c;
	}
	function point(e: PointerEvent) {
		const r = canvas!.getBoundingClientRect();
		return [((e.clientX - r.left) * canvas!.width) / r.width, ((e.clientY - r.top) * canvas!.height) / r.height];
	}
	function down(e: PointerEvent) {
		drawing = true;
		canvas!.setPointerCapture(e.pointerId);
		const [x, y] = point(e);
		const c = ctx();
		c.beginPath();
		c.moveTo(x, y);
		c.lineTo(x + 0.1, y + 0.1);
		c.stroke();
		drawn = true;
	}
	function move(e: PointerEvent) {
		if (!drawing) return;
		const [x, y] = point(e);
		const c = ctx();
		c.lineTo(x, y);
		c.stroke();
	}
	function up() {
		drawing = false;
	}
	function clear() {
		ctx().clearRect(0, 0, canvas!.width, canvas!.height);
		drawn = false;
	}

	async function sign(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			info = await api.post<Info>(`/public/sign/${encodeURIComponent(token)}`, { signed_name: typed, signature: canvas!.toDataURL('image/png'), agree });
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
	async function decline(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			info = await api.post<Info>(`/public/sign/${encodeURIComponent(token)}/decline`, { reason });
			declining = false;
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
	const STATUS: Record<string, string> = { pending: 'not signed yet', viewed: 'not signed yet', signed: 'signed', declined: 'declined' };
</script>

<svelte:head><title>{info ? `Sign ${info.title}` : 'Sign'}</title><meta name="robots" content="noindex" /></svelte:head>

<main class="min-h-screen bg-slate-50 px-4 py-8">
	<div class="mx-auto max-w-4xl space-y-5">
		{#if missing}
			<div class="mx-auto max-w-md rounded-3xl bg-white p-8 text-center shadow-g1">
				<h1 class="text-xl font-semibold text-slate-900">This link doesn't work</h1>
				<p class="mt-2 text-sm text-slate-600">It may be incomplete or replaced by a newer one. Use the latest email you received.</p>
			</div>
		{:else if info}
			<div>
				<p class="text-sm text-slate-500">{info.company}</p>
				<h1 class="text-2xl font-semibold text-slate-900">{info.title}</h1>
				{#if info.message}<p class="mt-2 whitespace-pre-line text-slate-700">{info.message}</p>{/if}
			</div>

			{#if info.status === 'signed' || info.request_status === 'completed'}
				<div class="rounded-2xl bg-emerald-50 px-5 py-4 text-emerald-900" role="status" data-done>
					<p class="font-semibold">{info.request_status === 'completed' ? 'Everyone has signed.' : 'Thank you, you have signed.'}</p>
					<p class="text-sm">{info.request_status === 'completed' ? 'The signed copy has been emailed to you.' : 'We will email you the signed copy when everyone has signed.'}</p>
					{#if info.signed_available}<a class="mt-2 inline-block text-sm font-medium underline" href={docUrl} target="_blank" rel="noopener">Open the signed copy</a>{/if}
				</div>
			{:else if info.status === 'declined'}
				<div class="rounded-2xl bg-slate-100 px-5 py-4 text-slate-700" role="status">You declined to sign. The sender has been told.</div>
			{:else if info.request_status !== 'sent'}
				<div class="rounded-2xl bg-slate-100 px-5 py-4 text-slate-700" role="status">
					{info.request_status === 'expired' ? 'This link has expired. Ask the sender for a new one.' : info.request_status === 'cancelled' ? 'The sender cancelled this request.' : 'This document is no longer waiting for signatures.'}
				</div>
			{/if}

			<div class="overflow-hidden rounded-2xl bg-white shadow-g1">
				<iframe src={docUrl} title={info.title} class="h-[70vh] w-full border-0"></iframe>
				<p class="border-t border-slate-100 px-4 py-2 text-xs text-slate-500">Can't see it? <a class="underline" href={docUrl} target="_blank" rel="noopener">Open the PDF</a>. Fingerprint (SHA-256): <span class="font-mono break-all">{info.fingerprint}</span></p>
			</div>

			{#if info.others.length}
				<p class="text-sm text-slate-600">Also signing: {info.others.map((o) => `${o.name} (${STATUS[o.status]})`).join(', ')}</p>
			{/if}

			{#if info.request_status === 'sent' && info.status === 'viewed'}
				{#if !declining}
					<form class="space-y-4 rounded-2xl bg-white p-5 shadow-g1" onsubmit={sign} aria-label="Sign">
						<h2 class="text-lg font-semibold text-slate-900">Sign as {info.name}</h2>
						<div><label class="label" for="sg-name">Your full name</label><input id="sg-name" class="input" required maxlength="200" autocomplete="name" bind:value={typed} /></div>
						<div>
							<div class="mb-1 flex items-center justify-between"><span class="label mb-0">Your signature</span><button type="button" class="text-xs text-slate-500 hover:text-slate-800" onclick={clear}>Clear</button></div>
							<canvas
								bind:this={canvas}
								width="900"
								height="240"
								class="h-32 w-full touch-none rounded-xl border border-dashed border-slate-300 bg-slate-50"
								aria-label="Draw your signature"
								onpointerdown={down}
								onpointermove={move}
								onpointerup={up}
								onpointerleave={up}
							></canvas>
							<p class="mt-1 text-xs text-slate-500">Draw with your mouse, finger or pen.</p>
						</div>
						<label class="flex items-start gap-2 text-sm text-slate-700">
							<input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" bind:checked={agree} required />
							I have read {info.title} and sign it electronically. My electronic signature is as binding as one on paper.
						</label>
						{#if error}<p class="text-sm text-red-700" role="alert">{error}</p>{/if}
						<div class="flex flex-wrap gap-3">
							<button class="btn btn-primary" disabled={busy || !drawn || !agree}>{busy ? 'Signing…' : 'Sign'}</button>
							<button type="button" class="btn btn-ghost" onclick={() => (declining = true)}>Decline</button>
						</div>
					</form>
				{:else}
					<form class="space-y-3 rounded-2xl bg-white p-5 shadow-g1" onsubmit={decline} aria-label="Decline">
						<h2 class="text-lg font-semibold text-slate-900">Decline to sign</h2>
						<div><label class="label" for="sg-reason">Why? <span class="font-normal text-slate-400">(optional, shared with the sender)</span></label><textarea id="sg-reason" class="input" rows="3" maxlength="1000" bind:value={reason}></textarea></div>
						<div class="flex gap-3">
							<button class="btn btn-primary" disabled={busy}>Decline</button>
							<button type="button" class="btn btn-ghost" onclick={() => (declining = false)}>Back</button>
						</div>
					</form>
				{/if}
			{/if}
		{:else}
			<p class="text-center text-sm text-slate-400">Loading…</p>
		{/if}
	</div>
</main>
