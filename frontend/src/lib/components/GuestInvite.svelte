<script lang="ts">
	// Invite someone from outside the company into a chat or a workgroup: they get an email
	// to set a password and see only the chats and groups they're invited to.
	import { api } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let { endpoint, oninvited }: { endpoint: string; oninvited?: () => void } = $props();
	let open = $state(false);
	let form = $state({ email: '', first_name: '', last_name: '' });
	let busy = $state(false);
	async function send(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		try {
			const r = await api.post<{ invited: boolean }>(endpoint, form);
			toasts.success(r.invited ? `${form.email} is invited as a guest` : `${form.email} is added`);
			form = { email: '', first_name: '', last_name: '' };
			open = false;
			oninvited?.();
		} catch (err) {
			toasts.error(err);
		} finally {
			busy = false;
		}
	}
</script>

{#if open}
	<form
		class="space-y-2 rounded-xl border border-slate-200 p-3 text-sm"
		onsubmit={send}
		data-guest-invite
	>
		<p class="text-xs text-slate-500">
			A guest sees only what they're invited into, never the rest of the company.
		</p>
		<input
			class="input"
			type="email"
			required
			placeholder="Their email"
			aria-label="Guest's email"
			bind:value={form.email}
		/>
		<div class="grid grid-cols-2 gap-2">
			<input
				class="input"
				maxlength="100"
				placeholder="First name"
				aria-label="Guest's first name"
				bind:value={form.first_name}
			/>
			<input
				class="input"
				maxlength="100"
				placeholder="Last name"
				aria-label="Guest's last name"
				bind:value={form.last_name}
			/>
		</div>
		<div class="flex gap-2">
			<button class="btn btn-primary" disabled={busy}>Invite</button><button
				type="button"
				class="btn btn-ghost"
				onclick={() => (open = false)}>Cancel</button
			>
		</div>
	</form>
{:else}
	<button type="button" class="text-sm text-brand-600 hover:underline" onclick={() => (open = true)}
		>+ Invite a guest from outside the company</button
	>
{/if}
