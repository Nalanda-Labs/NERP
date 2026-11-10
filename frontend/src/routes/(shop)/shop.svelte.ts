// The public shop's state: its settings and the visitor's cart (kept on this device).
import { api } from '#lib/api.ts';

export interface ShopInfo {
	name: string;
	intro: string | null;
	currency: string;
	company: string | null;
	terms: string | null;
	payment_instructions: string | null;
	categories: { name: string; slug: string }[];
}

export interface ShopProduct {
	slug: string;
	name: string;
	description: string | null;
	/** One unit, tax included. */
	price: string;
	tax_rate: string | null;
	unit: string;
	category: string | null;
	image: string | null;
	/** Units left when stock is counted. */
	stock: string | null;
	/** Products with variants: how many (`price` is the lowest). */
	variants: number;
	/** A variant: its values of the variant properties (property id → value). */
	values?: Record<string, string>;
}

export interface CartLine {
	slug: string;
	name: string;
	price: string;
	unit: string;
	image: string | null;
	quantity: number;
	/** Units left when the product was added (null: not counted). */
	stock: number | null;
}

const KEY = 'shop.cart';

function read(): CartLine[] {
	try {
		const v = JSON.parse(localStorage.getItem(KEY) ?? '[]');
		return Array.isArray(v) ? v.filter((l) => l && typeof l.slug === 'string' && l.quantity > 0) : [];
	} catch {
		return [];
	}
}

class Shop {
	info = $state<ShopInfo | null>(null);
	closed = $state(false);
	cart = $state<CartLine[]>(read());

	count = $derived(this.cart.reduce((a, l) => a + l.quantity, 0));
	total = $derived(this.cart.reduce((a, l) => a + Number(l.price) * l.quantity, 0));

	async load() {
		if (this.info || this.closed) return;
		try {
			this.info = await api.get<ShopInfo>('/public/shop');
		} catch {
			this.closed = true;
		}
	}

	private save() {
		try {
			localStorage.setItem(KEY, JSON.stringify(this.cart));
		} catch {
			/* private mode: the cart lasts for this visit */
		}
	}

	/** The most that can go in the cart. */
	max(stock: number | null) {
		return Math.min(999, stock ?? 999);
	}

	add(p: ShopProduct, quantity = 1) {
		const stock = p.stock === null ? null : Number(p.stock);
		const line = this.cart.find((l) => l.slug === p.slug);
		if (line) {
			line.quantity = Math.min(this.max(stock), line.quantity + quantity);
			Object.assign(line, { name: p.name, price: p.price, image: p.image, stock });
		} else {
			this.cart.push({ slug: p.slug, name: p.name, price: p.price, unit: p.unit, image: p.image, quantity: Math.min(this.max(stock), quantity), stock });
		}
		this.save();
	}

	set(slug: string, quantity: number) {
		const line = this.cart.find((l) => l.slug === slug);
		if (!line) return;
		if (quantity <= 0) this.cart = this.cart.filter((l) => l.slug !== slug);
		else line.quantity = Math.min(this.max(line.stock), Math.floor(quantity));
		this.save();
	}

	clear() {
		this.cart = [];
		this.save();
	}
}

export const shop = new Shop();
