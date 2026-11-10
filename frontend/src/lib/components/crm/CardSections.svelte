<script lang="ts">
	// A record's card fields, grouped into the sections of its type's card layout.
	import { crm, formatAddress, invoiceOverdue, money, optionLabel, ufText, type CardLayoutInfo, type CrmDetail, type CrmType } from '#lib/crm.svelte.ts';
	import { formatDateTime, formatRelative } from '#lib/format.ts';
	import Icon from '../Icon.svelte';
	import CardEditor from './CardEditor.svelte';

	let { type, item, info, onchange }: { type: CrmType; item: CrmDetail; info: CardLayoutInfo; onchange: (i: CardLayoutInfo) => void } = $props();

	let editing = $state(false);
	const label = (code: string) => info.fields.find((f) => f.code === code)?.label ?? code;
	const day = (d: string) => new Date(d + 'T00:00').toLocaleDateString();
	/** Fields that take the full width of a section. */
	const WIDE = new Set(['contact_details', 'address', 'comments', 'source_details', 'utm']);

	function empty(code: string): boolean {
		switch (code) {
			case 'contact_details': return !item.multifields.length;
			case 'utm': return !item.utm || !Object.keys(item.utm).length;
			case 'probability': return item.probability === null;
			default: {
				if (code.startsWith('uf.')) {
					const def = type.custom_fields.find((d) => `uf.${d.code}` === code);
					return !def || !ufText(def, item.uf[def.code]);
				}
				const v = (item as unknown as Record<string, unknown>)[code];
				return v === null || v === undefined || v === '';
			}
		}
	}
	// Campaign tags come only from web forms, so an empty one is never worth a row.
	const visible = (fields: string[]) => fields.filter((c) => !((info.layout.hide_empty || c === 'utm') && empty(c)));
</script>

{#snippet value(code: string)}
	{#if code === 'close_date'}
		<span class="flex items-center gap-2">
			{item.close_date ? day(item.close_date) : '—'}
			{#if invoiceOverdue(type, item)}<span class="rounded bg-red-50 px-1.5 py-0.5 text-xs font-medium text-red-700">Overdue</span>{/if}
		</span>
	{:else if code === 'probability'}{item.probability !== null ? `${item.probability}%` : '—'}
	{:else if code === 'birthdate'}{item.birthdate ? day(item.birthdate) : '—'}
	{:else if code === 'company_kind' || code === 'industry' || code === 'source'}{optionLabel(item[code]) || '—'}
	{:else if code === 'employees'}{item.employees ?? '—'}
	{:else if code === 'annual_revenue'}{item.annual_revenue ? money(item.annual_revenue, crm.config?.base_currency) : '—'}
	{:else if code === 'source_details'}<span class="break-words">{item.source_details || '—'}</span>
	{:else if code === 'utm'}{item.utm ? Object.entries(item.utm).map(([k, v]) => `${k}: ${v}`).join(' · ') : '—'}
	{:else if code === 'address'}{item.address ? formatAddress(item.address) : '—'}
	{:else if code === 'comments'}<span class="whitespace-pre-line">{item.comments || '—'}</span>
	{:else if code === 'contact_details'}
		{#if item.multifields.length}
			<ul class="grid gap-x-6 gap-y-1.5 sm:grid-cols-2">
				{#each item.multifields as m (m.id)}
					<li class="min-w-0">
						<span class="text-xs text-slate-500">{optionLabel(m.kind === 'im' ? m.value_type : m.kind)}{m.kind !== 'im' ? ` · ${m.value_type}` : ''}</span>
						<span class="block truncate">
							{#if m.kind === 'phone'}<a class="text-brand-600 hover:underline" href="tel:{m.value}">{m.value}</a>
							{:else if m.kind === 'email'}<a class="text-brand-600 hover:underline" href="mailto:{m.value}">{m.value}</a>
							{:else if m.kind === 'web'}<a class="text-brand-600 hover:underline" href={m.value.startsWith('http') ? m.value : `https://${m.value}`} target="_blank" rel="noopener noreferrer">{m.value}</a>
							{:else}{m.value}{/if}
						</span>
					</li>
				{/each}
			</ul>
		{:else}—{/if}
	{:else if code.startsWith('uf.')}
		{@const def = type.custom_fields.find((d) => `uf.${d.code}` === code)}
		{(def && ufText(def, item.uf[def.code])) || '—'}
	{:else}{String((item as unknown as Record<string, unknown>)[code] ?? '—')}{/if}
{/snippet}

{#each info.layout.sections as s, i (s.id)}
	{@const fields = visible(s.fields)}
	{#if fields.length || i === 0}
		<section class="card" data-card-section={s.id} aria-labelledby="cs-{s.id}">
			<div class="flex items-center gap-2 border-b border-slate-100 px-5 py-3">
				<h2 id="cs-{s.id}" class="flex-1 font-medium text-slate-900">{s.title}</h2>
				{#if i === 0}
					<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => (editing = true)} title="Choose which fields show and in which order"><Icon name="settings" class="size-3.5" /> Customize</button>
				{/if}
			</div>
			<dl class="grid gap-x-6 gap-y-3 p-5 text-sm sm:grid-cols-2">
				{#each fields as code (code)}
					<div class={WIDE.has(code) ? 'sm:col-span-2' : ''} data-field={code}>
						<dt class={fields.length === 1 && label(code) === s.title ? 'sr-only' : 'text-slate-500'}>{label(code)}</dt>
						<dd>{@render value(code)}</dd>
					</div>
					{#if code === 'close_date' && type.kind === 'invoice' && item.stage_semantics === 'success' && item.closed_at}
						<div><dt class="text-slate-500">Paid</dt><dd>{new Date(item.closed_at).toLocaleDateString()}</dd></div>
					{/if}
				{/each}
				{#if i === 0}
					<div class="text-xs text-slate-400 sm:col-span-2">Created {formatDateTime(item.created_at)} · updated {formatRelative(item.updated_at)}</div>
				{/if}
			</dl>
		</section>
	{/if}
{/each}

{#if editing}
	<CardEditor {type} {info} onclose={() => (editing = false)} onsaved={(i) => ((editing = false), onchange(i))} />
{/if}
