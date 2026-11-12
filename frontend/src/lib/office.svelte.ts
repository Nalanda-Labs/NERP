// Which files open in the office editor (Collabora), asked once per page load.
import { api } from '#lib/api.ts';

class Office {
	editable = $state<string[]>([]);
	#asked = false;
	load() {
		if (this.#asked) return;
		this.#asked = true;
		api.get<{ editing: boolean; editable: string[] }>('/office/status').then((s) => (this.editable = s.editing ? s.editable : [])).catch(() => {});
	}
	/** Whether a file of this name opens in the editor. */
	opens(name: string): boolean {
		this.load();
		return this.editable.includes(name.split('.').pop()?.toLowerCase() ?? '');
	}
}

export const office = new Office();
