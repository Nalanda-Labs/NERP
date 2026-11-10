import { ApiError } from './api';

export interface Toast {
	id: number;
	kind: 'success' | 'error' | 'info';
	message: string;
}

let next = 1;

class Toasts {
	items = $state<Toast[]>([]);

	push(kind: Toast['kind'], message: string) {
		const id = next++;
		this.items.push({ id, kind, message });
		setTimeout(() => this.dismiss(id), kind === 'error' ? 6000 : kind === 'info' ? 5000 : 3000);
	}

	info(message: string) {
		this.push('info', message);
	}

	success(message: string) {
		this.push('success', message);
	}

	error(e: unknown) {
		const message =
			e instanceof ApiError ? e.message : e instanceof Error ? e.message : 'Something went wrong';
		this.push('error', message.charAt(0).toUpperCase() + message.slice(1));
	}

	dismiss(id: number) {
		this.items = this.items.filter((t) => t.id !== id);
	}
}

export const toasts = new Toasts();
