// Shapes and labels shared by the inventory pages.
import { api } from '#lib/api.ts';
import { goto } from '$app/navigation';
import { toasts } from '#lib/toast.svelte.ts';

export interface Warehouse {
	id: string;
	name: string;
	address: string | null;
	is_default: boolean;
	is_active: boolean;
	products: number;
	units: string;
}

export type DocKind = 'receipt' | 'writeoff' | 'transfer' | 'count';

export const KIND: Record<DocKind, { name: string; action: string; hint: string }> = {
	receipt: { name: 'Receipt', action: 'Receive goods', hint: 'Goods arriving in a warehouse, e.g. from a supplier.' },
	writeoff: { name: 'Write-off', action: 'Write off', hint: 'Goods leaving a warehouse: damaged, lost or used.' },
	transfer: { name: 'Transfer', action: 'Transfer', hint: 'Goods moving from one warehouse to another.' },
	count: { name: 'Stock count', action: 'Stock count', hint: 'Set each listed product to the quantity actually found.' }
};

export const STATUS: Record<string, [string, string]> = {
	draft: ['Draft', 'bg-slate-100 text-slate-700'],
	posted: ['Posted', 'bg-emerald-50 text-emerald-800'],
	cancelled: ['Cancelled', 'bg-slate-100 text-slate-500']
};

export const REASON: Record<string, string> = {
	opening: 'Opening balance',
	document: 'Document',
	document_cancelled: 'Document cancelled',
	order: 'Shop order',
	order_failed: 'Order not placed',
	order_cancelled: 'Order cancelled',
	order_reopened: 'Order reopened',
	product_form: 'Changed on the product'
};

/** "12" from "12.0000". */
export const qty = (v: string | number | null | undefined) => (v === null || v === undefined ? '' : String(Number(v)));

/** Starts a draft document and opens it. */
export async function newDocument(kind: DocKind) {
	try {
		const d = await api.post<{ id: string }>('/inventory/documents', { kind });
		goto(`/inventory/documents/${d.id}`);
	} catch (e) {
		toasts.error(e);
	}
}
