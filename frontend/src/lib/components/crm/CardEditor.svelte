<script lang="ts">
	// Arrange a record type's card: sections (add, rename, reorder, remove), the fields in
	// each (reorder, move, hide), and whether empty fields show. Saved for the current user,
	// or — for CRM admins — for everyone.
	import { untrack } from 'svelte';
	import { api } from '#lib/api.ts';
	import { setCardLayout, type CardLayout, type CardLayoutInfo, type CrmType } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import Modal from '../Modal.svelte';

	let { type, info, onclose, onsaved }: { type: CrmType; info: CardLayoutInfo; onclose: () => void; onsaved: (i: CardLayoutInfo) => void } = $props();

	let open = $state(true);
	// The editor works on a copy taken when it opens.
	let draft = $state<CardLayout>(untrack(() => structuredClone($state.snapshot(info.layout))));
	let scope = $state<'mine' | 'common'>(untrack(() => (info.scope === 'common' && info.can_edit_common ? 'common' : 'mine')));
	let saving = $state(false);
	const field = (code: string) => info.fields.find((f) => f.code === code);
	const label = (code: string) => field(code)?.label ?? code;

	function shift<T>(list: T[], i: number, by: number) {
		const j = i + by;
		if (j < 0 || j >= list.length) return;
		[list[i], list[j]] = [list[j], list[i]];
	}
	function moveTo(from: number, i: number, to: number) {
		const [code] = draft.sections[from].fields.splice(i, 1);
		draft.sections[to].fields.push(code);
	}
	function hide(s: number, i: number) {
		const [code] = draft.sections[s].fields.splice(i, 1);
		draft.hidden.push(code);
	}
	function show(code: string, to: number) {
		draft.hidden = draft.hidden.filter((c) => c !== code);
		draft.sections[to].fields.push(code);
	}
	function addSection() {
		draft.sections.push({ id: `s${Date.now().toString(36)}`, title: 'New section', fields: [] });
	}
	function removeSection(i: number) {
		const [s] = draft.sections.splice(i, 1);
		// Its fields move to the section before it (or the first one), not out of sight.
		draft.sections[Math.max(0, i - 1)].fields.push(...s.fields);
	}

	async function save() {
		saving = true;
		try {
			const r = await api.put<CardLayoutInfo>(`/crm/${type.code}/card-layout`, { layout: draft, scope });
			setCardLayout(type.code, r);
			toasts.success(scope === 'common' ? 'Card saved for everyone' : 'Card saved for you');
			onsaved(r);
		} catch (e) {
			toasts.error(e);
		} finally {
			saving = false;
		}
	}
	async function reset(which: 'mine' | 'common') {
		const msg = which === 'mine' ? 'Go back to the card everyone uses?' : 'Reset the card for everyone to the built-in layout?';
		if (!confirm(msg)) return;
		try {
			const r = await api.del<CardLayoutInfo>(`/crm/${type.code}/card-layout?scope=${which}`);
			setCardLayout(type.code, r);
			toasts.success('Card reset');
			onsaved(r);
		} catch (e) {
			toasts.error(e);
		}
	}
</script>

<Modal bind:open title="Customize the {type.name.toLowerCase()} card" size="lg" onclose={onclose}>
	<div class="space-y-4 text-sm">
		<p class="text-slate-600">Group the fields into sections and put them in order. The record page and its form follow this layout. Name, amount, stage, client and responsible person always stay at the top.</p>

		{#each draft.sections as s, si (s.id)}
			<section class="rounded-xl border border-slate-200" aria-label="Section {s.title}">
				<div class="flex items-center gap-2 border-b border-slate-100 bg-slate-50 px-3 py-2">
					<input class="input h-8 flex-1 font-medium" maxlength="60" bind:value={s.title} aria-label="Section title" />
					<button type="button" class="rounded px-1.5 py-1 hover:bg-white disabled:opacity-30" disabled={si === 0} aria-label="Move section up" onclick={() => shift(draft.sections, si, -1)}>↑</button>
					<button type="button" class="rounded px-1.5 py-1 hover:bg-white disabled:opacity-30" disabled={si === draft.sections.length - 1} aria-label="Move section down" onclick={() => shift(draft.sections, si, 1)}>↓</button>
					{#if draft.sections.length > 1}
						<button type="button" class="rounded p-1 text-slate-400 hover:bg-white hover:text-red-600" aria-label="Remove section {s.title}" title="Remove the section (its fields move to the one above)" onclick={() => removeSection(si)}><Icon name="trash" class="size-4" /></button>
					{/if}
				</div>
				<ul class="divide-y divide-slate-50">
					{#each s.fields as code, fi (code)}
						<li class="flex items-center gap-2 px-3 py-1.5" data-field={code}>
							<span class="flex-1 text-slate-800">{label(code)}{#if field(code)?.required}<span class="ml-1 text-xs text-slate-400">required</span>{/if}</span>
							<button type="button" class="rounded px-1.5 hover:bg-slate-100 disabled:opacity-30" disabled={fi === 0} aria-label="Move {label(code)} up" onclick={() => shift(s.fields, fi, -1)}>↑</button>
							<button type="button" class="rounded px-1.5 hover:bg-slate-100 disabled:opacity-30" disabled={fi === s.fields.length - 1} aria-label="Move {label(code)} down" onclick={() => shift(s.fields, fi, 1)}>↓</button>
							{#if draft.sections.length > 1}
								<select class="input h-7 w-36 py-0 text-xs" aria-label="Move {label(code)} to section" value="" onchange={(e) => { const to = Number(e.currentTarget.value); e.currentTarget.value = ''; moveTo(si, fi, to); }}>
									<option value="" disabled>Move to…</option>
									{#each draft.sections as t, ti (t.id)}{#if ti !== si}<option value={ti}>{t.title}</option>{/if}{/each}
								</select>
							{/if}
							<button type="button" class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 disabled:opacity-30" disabled={field(code)?.required} aria-label="Hide {label(code)}" title={field(code)?.required ? 'Required fields cannot be hidden' : 'Hide'} onclick={() => hide(si, fi)}><Icon name="eye" class="size-4" /></button>
						</li>
					{:else}
						<li class="px-3 py-2 text-xs text-slate-400">No fields — move some here, or remove the section.</li>
					{/each}
				</ul>
			</section>
		{/each}
		<button type="button" class="btn btn-ghost text-sm" onclick={addSection}><Icon name="plus" class="size-4" /> Add section</button>

		<div>
			<p class="label">Hidden fields</p>
			{#if draft.hidden.length}
				<div class="flex flex-wrap gap-1.5">
					{#each draft.hidden as code (code)}
						<button type="button" class="inline-flex items-center gap-1 rounded-full bg-slate-100 px-2.5 py-1 text-xs text-slate-700 hover:bg-brand-50 hover:text-brand-800" title="Show it again, at the end of the last section" onclick={() => show(code, draft.sections.length - 1)}>
							<Icon name="plus" class="size-3" /> {label(code)}
						</button>
					{/each}
				</div>
			{:else}
				<p class="text-xs text-slate-500">None. Hidden fields keep their values; they just aren't shown or edited on the card.</p>
			{/if}
		</div>

		<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.hide_empty} /> On the record page, leave out fields without a value</label>

		<fieldset class="space-y-1.5 rounded-xl bg-slate-50 p-3">
			<legend class="sr-only">Who sees this layout</legend>
			<label class="flex items-center gap-2"><input type="radio" class="text-brand-600" value="mine" bind:group={scope} /> Just for me</label>
			{#if info.can_edit_common}
				<label class="flex items-center gap-2"><input type="radio" class="text-brand-600" value="common" bind:group={scope} /> For everyone <span class="text-xs text-slate-500">(people with their own layout keep it)</span></label>
			{/if}
		</fieldset>
	</div>
	{#snippet footer()}
		{#if info.scope === 'mine'}
			<button class="btn btn-ghost mr-auto" onclick={() => reset('mine')}>Use the shared card</button>
		{:else if info.scope === 'common' && info.can_edit_common}
			<button class="btn btn-ghost mr-auto" onclick={() => reset('common')}>Reset to default</button>
		{/if}
		<button class="btn btn-secondary" onclick={onclose}>Cancel</button>
		<button class="btn btn-primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
	{/snippet}
</Modal>
