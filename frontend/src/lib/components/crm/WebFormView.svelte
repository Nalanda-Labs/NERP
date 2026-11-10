<script lang="ts" module>
	export type PublicField = { key: string; label: string; kind: string; required: boolean; options: string[]; placeholder: string };
	export type PublicForm = {
		name: string;
		company: string;
		settings: { title?: string; description?: string; button?: string; success_message?: string; redirect_url?: string; color?: string; consent_text?: string };
		fields: PublicField[];
		token: string;
	};
</script>

<script lang="ts">
	// Renders a web form for visitors (hosted page, embed) and as the editor's preview.
	let {
		form,
		preview = false,
		onsubmit
	}: { form: PublicForm; preview?: boolean; onsubmit?: (values: Record<string, string | boolean>, website: string, consent: boolean) => Promise<string | null> } = $props();

	let values = $state<Record<string, string | boolean>>({});
	let website = $state('');
	let consent = $state(false);
	let sending = $state(false);
	let error = $state<string | null>(null);
	let done = $state<string | null>(null);
	const color = $derived(form.settings.color || '#0b57d0');

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (preview || !onsubmit) return;
		sending = true;
		error = null;
		try {
			done = await onsubmit($state.snapshot(values), website, consent);
		} catch (err) {
			error = (err as Error).message;
		} finally {
			sending = false;
		}
	}
</script>

<div class="nf" style="--nf-color:{color}">
	{#if done}
		<div class="rounded-2xl bg-white p-8 text-center shadow-sm">
			<div class="mx-auto mb-3 flex size-12 items-center justify-center rounded-full text-2xl text-white" style="background:{color}">✓</div>
			<p class="text-base text-slate-800">{done}</p>
		</div>
	{:else}
		<form class="space-y-4 rounded-2xl bg-white p-6 shadow-sm sm:p-8" onsubmit={submit} novalidate>
			{#if form.settings.title}<h1 class="text-xl font-semibold text-slate-900">{form.settings.title}</h1>{/if}
			{#if form.settings.description}<p class="text-sm whitespace-pre-line text-slate-600">{form.settings.description}</p>{/if}
			{#each form.fields as f (f.key)}
				<div>
					{#if f.kind === 'checkbox'}
						<label class="flex items-start gap-2 text-sm text-slate-700">
							<input type="checkbox" class="mt-0.5 rounded border-slate-300" style="accent-color:{color}" checked={!!values[f.key]} onchange={(e) => (values[f.key] = e.currentTarget.checked)} />
							<span>{f.label}{#if f.required}<span class="text-red-600"> *</span>{/if}</span>
						</label>
					{:else}
						<label class="mb-1 block text-sm font-medium text-slate-700" for="nf-{f.key}">{f.label}{#if f.required}<span class="text-red-600"> *</span>{/if}</label>
						{#if f.kind === 'textarea'}
							<textarea id="nf-{f.key}" class="nf-input" rows="4" placeholder={f.placeholder} required={f.required} bind:value={values[f.key] as string}></textarea>
						{:else if f.kind === 'select'}
							<select id="nf-{f.key}" class="nf-input" required={f.required} bind:value={values[f.key] as string}>
								<option value="">Choose…</option>
								{#each f.options as o (o)}<option>{o}</option>{/each}
							</select>
						{:else}
							<input
								id="nf-{f.key}"
								class="nf-input"
								type={f.kind === 'email' ? 'email' : f.kind === 'phone' ? 'tel' : f.kind === 'number' ? 'number' : f.kind === 'date' ? 'date' : 'text'}
								autocomplete={f.kind === 'email' ? 'email' : f.kind === 'phone' ? 'tel' : 'on'}
								placeholder={f.placeholder}
								required={f.required}
								bind:value={values[f.key] as string}
							/>
						{/if}
					{/if}
				</div>
			{/each}
			<!-- Decoy for bots: hidden from people and screen readers. -->
			<div aria-hidden="true" style="position:absolute;left:-10000px;top:auto;width:1px;height:1px;overflow:hidden">
				<label>Website <input tabindex="-1" autocomplete="off" bind:value={website} /></label>
			</div>
			{#if form.settings.consent_text}
				<label class="flex items-start gap-2 text-xs text-slate-600">
					<input type="checkbox" class="mt-0.5 rounded border-slate-300" style="accent-color:{color}" bind:checked={consent} />
					<span class="whitespace-pre-line">{form.settings.consent_text}</span>
				</label>
			{/if}
			{#if error}<p class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-800" role="alert">{error}</p>{/if}
			<button class="w-full rounded-full px-5 py-2.5 text-sm font-medium text-white shadow-sm disabled:opacity-60" style="background:{color}" disabled={sending}>{sending ? 'Sending…' : form.settings.button || 'Send'}</button>
		</form>
	{/if}
</div>

<style>
	.nf :global(.nf-input) {
		width: 100%;
		border-radius: 0.5rem;
		border: 1px solid rgb(203 213 225);
		padding: 0.55rem 0.75rem;
		font-size: 0.9rem;
		background: white;
	}
	.nf :global(.nf-input:focus) {
		outline: 2px solid var(--nf-color);
		outline-offset: -1px;
		border-color: transparent;
	}
</style>
