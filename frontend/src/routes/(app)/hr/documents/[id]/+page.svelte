<script lang="ts">
	// One HR document: the PDF, its steps, and my part (approve, or sign / decline); HR can
	// withdraw it or declare a signed one invalid.
	import { page } from '$app/state';
	import { onDestroy, onMount } from 'svelte';
	import { api, requestBlob } from '#lib/api.ts';
	import { formatDateTime } from '#lib/format.ts';
	import SignaturePad from '#lib/components/SignaturePad.svelte';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { PERM } from '#lib/types.ts';

	type Step = { position: number; name: string; role: string; status: string; acted_at: string | null; comment: string | null; signed_name: string | null };
	type Doc = { id: string; title: string; employee_name: string; status: string; overdue: boolean; due_on: string | null; sha256: string; signed_sha256: string | null; revoke_reason: string | null; my_turn: string | null; steps: Step[] };
	const id = $derived(page.params.id!);
	let d = $state<Doc | null>(null);
	let pdf = $state<string | null>(null);
	let typed = $state(session.user ? `${session.user.first_name} ${session.user.last_name}` : '');
	let agree = $state(false);
	let drawn = $state(false);
	let comment = $state('');
	let pad: SignaturePad | undefined = $state();
	const ROLE: Record<string, string> = { approver: 'Approves', company: 'Signs for the company', employee: 'Signs as the employee' };
	const STEP: Record<string, string> = { waiting: 'Waiting', pending: 'Their turn', done: 'Done', declined: 'Declined' };
	const STATUS: Record<string, string> = { approval: 'Being approved', signing: 'Being signed', signed: 'Signed', declined: 'Declined', revoked: 'Not valid' };

	async function load() {
		d = await api.get<Doc>(`/hr/docs/${id}`);
		if (pdf) URL.revokeObjectURL(pdf);
		pdf = URL.createObjectURL(await requestBlob('GET', `/hr/docs/${id}/file`));
	}
	onMount(() => load().catch(toasts.error));
	onDestroy(() => pdf && URL.revokeObjectURL(pdf));
	async function act(path: string, body: object) {
		try {
			d = await api.post<Doc>(`/hr/docs/${id}/${path}`, body);
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}
	const sign = () => act('sign', { signed_name: typed, signature: pad!.png(), agree });
	async function revoke() {
		const reason = prompt(d?.status === 'signed' ? 'Why is the signed document no longer valid?' : 'Why is it withdrawn?');
		if (reason?.trim()) act('revoke', { reason });
	}
</script>

<svelte:head><title>{d?.title ?? 'Document'} · NERP</title></svelte:head>

{#if d}
	<div class="space-y-4">
		<a href="/hr/documents" class="text-sm text-brand-600 hover:underline">‹ Documents to sign</a>
		<div class="flex flex-wrap items-center gap-3">
			<h1 class="text-2xl font-semibold text-slate-900">{d.title}</h1>
			<span class="text-slate-500">{d.employee_name}</span>
			<span class="badge {d.status === 'signed' ? 'bg-emerald-50 text-emerald-700' : d.status === 'revoked' || d.status === 'declined' ? 'bg-red-50 text-red-700' : 'bg-slate-100 text-slate-700'}" data-status={d.status}>{STATUS[d.status]}</span>
			{#if d.overdue}<span class="badge bg-red-50 text-red-700">Past its due date</span>{/if}
			{#if session.can(PERM.HR) && d.status !== 'revoked'}<button class="btn btn-ghost ml-auto hover:text-red-600" onclick={revoke}>{d.status === 'signed' ? 'Declare invalid' : 'Withdraw'}</button>{/if}
		</div>
		{#if d.revoke_reason}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-800">Not valid: {d.revoke_reason}</p>{/if}
		<div class="grid gap-4 lg:grid-cols-3">
			<div class="card overflow-hidden lg:col-span-2">{#if pdf}<iframe src={pdf} title={d.title} class="h-[70vh] w-full" data-pdf></iframe>{/if}</div>
			<div class="space-y-4">
				<section class="card p-4 text-sm" aria-label="Steps">
					<h2 class="mb-2 font-medium text-slate-900">Steps</h2>
					<ol class="space-y-2">
						{#each d.steps as s (s.position)}
							<li data-step={s.role}><p><span class="font-medium">{s.name}</span> · {ROLE[s.role]}</p><p class="text-xs {s.status === 'declined' ? 'text-red-600' : 'text-slate-500'}">{STEP[s.status]}{s.acted_at ? ` · ${formatDateTime(s.acted_at)}` : ''}{s.signed_name ? ` · signed as “${s.signed_name}”` : ''}{s.comment ? ` · “${s.comment}”` : ''}</p></li>
						{/each}
					</ol>
				</section>
				{#if d.my_turn === 'approver'}
					<section class="card space-y-2 p-4" aria-label="Approve">
						<textarea class="input" rows="2" aria-label="Comment" placeholder="A comment (optional)" bind:value={comment}></textarea>
						<div class="flex gap-2"><button class="btn btn-primary" onclick={() => act('approve', { approve: true, comment })}>Approve</button><button class="btn btn-secondary" onclick={() => act('approve', { approve: false, comment })}>Send back</button></div>
					</section>
				{:else if d.my_turn}
					<section class="card space-y-3 p-4 text-sm" aria-label="Sign">
						<div><label class="label" for="hs-name">Your full name</label><input id="hs-name" class="input" maxlength="200" bind:value={typed} /></div>
						<SignaturePad bind:this={pad} bind:drawn />
						<label class="flex items-start gap-2"><input type="checkbox" class="mt-0.5 rounded border-slate-300 text-brand-600" bind:checked={agree} /> I have read {d.title} and sign it electronically. My electronic signature is as binding as one on paper.</label>
						<div class="flex gap-2"><button class="btn btn-primary" disabled={!agree || !drawn || !typed.trim()} onclick={sign}>Sign</button><button class="btn btn-ghost hover:text-red-600" onclick={() => { const r = prompt('Why do you decline to sign?'); if (r !== null) act('decline', { reason: r }); }}>Decline</button></div>
					</section>
				{/if}
				<section class="card p-4 text-xs text-slate-500" aria-label="Fingerprint">
					<p>Fingerprint as sent (SHA-256): <code class="break-all">{d.sha256}</code></p>
					{#if d.signed_sha256}<p class="mt-1">Signed copy: <code class="break-all" data-signed-sha>{d.signed_sha256}</code></p><p class="mt-1">Anyone can check a copy at <code>/api/public/verify/&lt;fingerprint&gt;</code>.</p>{/if}
				</section>
			</div>
		</div>
	</div>
{/if}
