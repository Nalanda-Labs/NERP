<script lang="ts">
	// A certificate, ready to print (or save as PDF from the print dialog).
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Cert = { id: string; number: number; course_id: string; course_title: string; user_id: string; user_name: string; issued_at: string; score: number; max_score: number };
	let c = $state<{ certificate: Cert; company: string | null; can_manage: boolean } | null>(null);
	const id = $derived(page.params.id);
	$effect(() => {
		id;
		api.get<typeof c>(`/training/certificates/${id}`).then((r) => (c = r)).catch(toasts.error.bind(toasts));
	});
	async function revoke() {
		if (!confirm('Revoke this certificate? The course counts as not completed until it is completed again.')) return;
		await api.del(`/training/certificates/${id}`);
		goto(`/training/courses/${c!.certificate.course_id}/gradebook`);
	}
</script>

<svelte:head><title>Certificate · {c?.certificate.course_title ?? ''} · NERP</title></svelte:head>

{#if c}
	{@const x = c.certificate}
	<div class="mx-auto max-w-3xl space-y-4">
		<div class="flex items-center gap-2 print:hidden">
			<a href="/training/courses/{x.course_id}" class="flex-1 text-sm text-slate-500 hover:underline">‹ {x.course_title}</a>
			{#if c.can_manage}<button class="btn btn-ghost text-sm text-red-600" onclick={revoke}>Revoke</button>{/if}
			<button class="btn btn-primary" onclick={() => window.print()}>Print</button>
		</div>
		<div class="relative aspect-[1.414] rounded-2xl border-8 border-double border-brand-200 bg-white p-12 text-center shadow-g2 print:shadow-none" data-certificate>
			<p class="text-sm font-semibold tracking-[0.3em] text-brand-700 uppercase">Certificate of completion</p>
			<p class="mt-10 text-sm text-slate-500">This certifies that</p>
			<p class="mt-2 font-serif text-4xl text-slate-900" data-name>{x.user_name}</p>
			<p class="mt-6 text-sm text-slate-500">has completed the course</p>
			<p class="mt-2 text-2xl font-semibold text-slate-900" data-course>{x.course_title}</p>
			{#if x.max_score}<p class="mt-4 text-slate-600">with a score of {x.score} of {x.max_score} ({Math.round((x.score * 100) / x.max_score)} %)</p>{/if}
			<div class="absolute inset-x-12 bottom-10 flex items-end justify-between text-sm text-slate-500">
				<span>{new Date(x.issued_at).toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric' })}</span>
				<span class="text-lg font-medium text-slate-700">{c.company ?? ''}</span>
				<span>No. {x.number}</span>
			</div>
		</div>
	</div>
{/if}
