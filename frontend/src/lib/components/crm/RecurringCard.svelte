<script lang="ts">
	// "Repeat" on a deal, invoice or smart-process record: copies it on a schedule.
	import { goto } from '$app/navigation';
	import { api } from '#lib/api.ts';
	import Modal from '#lib/components/Modal.svelte';
	import RecurrenceEditor from '#lib/components/RecurrenceEditor.svelte';
	import { formatDateTime } from '#lib/format.ts';
	import type { Recurrence } from '#lib/tasks.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type View = { rule: Recurrence | null; summary?: string; due_days?: number | null; next_run_at?: string | null; last_run_at?: string | null; run_count?: number; is_active?: boolean };
	let { itemId, canEdit, dateLabel, onchanged }: { itemId: string; canEdit: boolean; dateLabel: string; onchanged?: () => void } = $props();

	let view = $state<View | null>(null);
	$effect(() => {
		const id = itemId;
		view = null;
		api.get<View>(`/crm/items/${id}/recurring`).then((v) => (view = v)).catch(() => {});
	});

	const details = $derived.by(() => {
		if (!view?.rule) return '';
		const parts = [view.is_active && view.next_run_at ? `Next copy ${formatDateTime(view.next_run_at)}` : 'Finished', `${view.run_count ?? 0} made on schedule`];
		if (view.due_days !== null && view.due_days !== undefined) parts.push(`${dateLabel.toLowerCase()} ${view.due_days} day${view.due_days === 1 ? '' : 's'} after each copy`);
		return parts.join(' · ');
	});
	let open = $state(false);
	let rule = $state<Recurrence | null>(null);
	let dueDays = $state<number | null>(null);
	let busy = $state(false);
	function edit() {
		rule = view?.rule ? { ...view.rule } : null;
		dueDays = view?.due_days ?? null;
		open = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			view = rule
				? await api.put<View>(`/crm/items/${itemId}/recurring`, { rule, due_days: dueDays === null || (dueDays as unknown) === '' ? null : Number(dueDays) })
				: await api.del<View>(`/crm/items/${itemId}/recurring`);
			open = false;
			toasts.success(rule ? 'It will repeat' : 'It no longer repeats');
			onchanged?.();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
	async function stop() {
		if (!confirm('Stop repeating? Copies already made stay.')) return;
		view = await api.del<View>(`/crm/items/${itemId}/recurring`);
		onchanged?.();
	}
	async function now() {
		busy = true;
		try {
			const r = await api.post<{ id: string; number: number; code: string }>(`/crm/items/${itemId}/recurring/now`, {});
			toasts.success(`#${r.number} created`);
			goto(`/crm/${r.code}/${r.id}`);
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

{#if view && (view.rule || canEdit)}
	<section class="card" aria-labelledby="rep-h" data-recurring>
		<div class="flex items-center justify-between border-b border-slate-100 px-5 py-3">
			<h2 id="rep-h" class="font-medium text-slate-900">Repeat</h2>
			{#if canEdit}<button class="text-sm font-medium text-brand-600 hover:underline" onclick={edit}>{view.rule ? 'Change' : 'Set up'}</button>{/if}
		</div>
		<div class="px-5 py-4 text-sm">
			{#if !view.rule}
				<p class="text-slate-500">Doesn't repeat. Set a schedule to get a copy of this record every week, month…</p>
			{:else}
				<p class="font-medium text-slate-800" data-summary>{view.summary}</p>
				<p class="mt-1 text-slate-600">{details}</p>
				{#if canEdit}
					<div class="mt-3 flex gap-2">
						<button class="btn btn-secondary px-3 py-1.5 text-sm" disabled={busy} onclick={now}>Create one now</button>
						<button class="btn btn-ghost px-3 py-1.5 text-sm" onclick={stop}>Stop repeating</button>
					</div>
				{/if}
			{/if}
		</div>
	</section>
{/if}

<Modal bind:open title="Repeat this record">
	<form id="rep-form" class="space-y-4" onsubmit={save}>
		<p class="text-sm text-slate-500">Each time, a copy of this record as it is then (client, products, amount, fields) starts in the first stage. Its robots run as usual.</p>
		<RecurrenceEditor bind:value={rule} />
		{#if rule}
			<div>
				<label class="label" for="rep-due">{dateLabel} of each copy</label>
				<div class="flex items-center gap-2 text-sm text-slate-600">
					<input id="rep-due" class="input w-24" type="number" min="0" max="3650" step="1" placeholder="—" bind:value={dueDays} /> days after it is made
				</div>
			</div>
		{/if}
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="rep-form" disabled={busy}>Save</button>
	{/snippet}
</Modal>
