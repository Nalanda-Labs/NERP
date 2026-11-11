// How the person arranged the app: sidebar items hidden or pinned, and the background.
import { api } from '#lib/api.ts';

export type Menu = { hidden: string[]; pinned: string[] };
type Prefs = { menu: Menu; background: string; own_menu: boolean; own_background: boolean };

export const BACKGROUNDS: Record<string, string> = {
	plain: '',
	sky: 'linear-gradient(180deg, #e0f2fe 0%, #f8fafc 60%)',
	sand: 'linear-gradient(180deg, #fdf3dc 0%, #fbfaf7 60%)',
	mint: 'linear-gradient(180deg, #d1fae5 0%, #f8fafc 60%)',
	dusk: 'linear-gradient(180deg, #ede9fe 0%, #fdf2f8 60%)',
	grid: 'radial-gradient(circle, #cbd5e1 1px, transparent 1px) 0 0 / 22px 22px, #f8fafc'
};

class Ui {
	prefs = $state<Prefs>({ menu: { hidden: [], pinned: [] }, background: 'plain', own_menu: false, own_background: false });
	editing = $state(false);

	async load() {
		this.prefs = await api.get<Prefs>('/me/ui').catch(() => this.prefs);
	}
	async save(body: { menu?: Menu | null; background?: string | null }) {
		this.prefs = await api.put<Prefs>('/me/ui', body);
	}
	/** CSS for the app's background. */
	get backgroundCss(): string {
		const b = this.prefs.background;
		if (BACKGROUNDS[b] !== undefined) return BACKGROUNDS[b] ? `background: ${BACKGROUNDS[b]}` : '';
		return `background: url(/api/files/${b}/content) center / cover fixed`;
	}
	toggle(kind: 'hidden' | 'pinned', href: string) {
		const m = { hidden: [...this.prefs.menu.hidden], pinned: [...this.prefs.menu.pinned] };
		m[kind] = m[kind].includes(href) ? m[kind].filter((h) => h !== href) : [...m[kind], href];
		return this.save({ menu: m });
	}
}

export const ui = new Ui();
