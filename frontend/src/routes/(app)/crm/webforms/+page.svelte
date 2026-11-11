<script lang="ts">
	// Web forms: build forms that create leads (or deals / smart-process records with their
	// contact), share or embed them, and see what came in.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import MultiUserPicker, { type PickedUser } from '#lib/components/MultiUserPicker.svelte';
	import { myMailboxes, type MailboxInfo } from '#lib/components/crm/EmailComposer.svelte';
	import WebFormView, { type PublicForm } from '#lib/components/crm/WebFormView.svelte';
	import { crm, itemLink } from '#lib/crm.svelte.ts';
	import { formatRelative } from '#lib/format.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import type { User } from '#lib/types.ts';

	type Field = { key: string; label: string; kind: string; required: boolean; maps_to: string; options: string[]; placeholder: string };
	type Form = {
		id: string;
		key: string;
		name: string;
		entity_type: number;
		category_id: string | null;
		stage_id: string | null;
		fields: Field[];
		settings: PublicForm['settings'];
		responsible: string[];
		reply_mailbox: string | null;
		reply_subject: string;
		reply_body: string;
		is_active: boolean;
		submissions: number;
	};
	type Summary = { id: string; key: string; name: string; entity_type: number; is_active: boolean; submissions: number; last_submission_at: string | null };

	const KINDS: [string, string][] = [['text', 'Text'], ['textarea', 'Long text'], ['email', 'Email'], ['phone', 'Phone'], ['number', 'Number'], ['date', 'Date'], ['select', 'Choice'], ['checkbox', 'Checkbox']];

	let items = $state<Summary[]>([]);
	let base = $state('');
	let draft = $state<(Omit<Form, 'id' | 'key' | 'submissions'> & { id: string | null; key: string | null; people: PickedUser[] }) | null>(null);
	let targets = $state<{ code: string; label: string }[]>([]);
	let mailboxes = $state<MailboxInfo[]>([]);
	let tab = $state<'build' | 'share' | 'results'>('build');
	let saving = $state(false);
	let results = $state<{ id: string; item_id: string | null; entity_type: number | null; item_title: string | null; data: { label: string; value: string }[]; utm: Record<string, string> | null; page_url: string | null; created_at: string }[]>([]);

	const types = $derived((crm.config?.types ?? []).filter((t) => t.code === 'lead' || (t.has_client && t.has_stages)));
	const type = $derived(types.find((t) => t.id === draft?.entity_type));
	const category = $derived(type?.categories.find((c) => c.id === draft?.category_id) ?? type?.categories.find((c) => c.is_default));

	async function load() {
		const r = await api.get<{ items: Summary[]; public_base: string }>('/crm/webforms');
		items = r.items;
		base = r.public_base;
	}
	onMount(async () => {
		await crm.load();
		await load();
		mailboxes = await myMailboxes();
	});

	async function loadTargets(et: number) {
		targets = (await api.get<{ items: typeof targets }>(`/crm/webforms/targets?entity_type=${et}`)).items;
	}

	async function open(id: string | null) {
		tab = 'build';
		if (!id) {
			template = 'contact';
			draft = {
				id: null, key: null, name: 'Contact us', entity_type: 1, category_id: null, stage_id: null, is_active: true,
				fields: [
					{ key: '', label: 'Name', kind: 'text', required: true, maps_to: 'first_name', options: [], placeholder: '' },
					{ key: '', label: 'Email', kind: 'email', required: true, maps_to: 'email', options: [], placeholder: 'you@example.com' },
					{ key: '', label: 'Phone', kind: 'phone', required: false, maps_to: 'phone', options: [], placeholder: '' },
					{ key: '', label: 'Message', kind: 'textarea', required: false, maps_to: 'comments', options: [], placeholder: '' }
				],
				settings: { title: 'Get in touch', description: 'Leave your details and we will get back to you within one working day.', button: 'Send', success_message: 'Thank you! We will be in touch soon.' },
				responsible: [], people: [], reply_mailbox: null, reply_subject: '', reply_body: ''
			};
		} else {
			const f = await api.get<Form>(`/crm/webforms/${id}`);
			const people: PickedUser[] = [];
			for (const u of f.responsible) {
				try {
					const x = await api.get<User>(`/users/${u}`);
					people.push({ id: u, name: x.full_name, avatar_file_id: x.avatar_file_id });
				} catch {
					/* gone */
				}
			}
			draft = { ...f, people };
		}
		await loadTargets(draft!.entity_type);
	}

	// Ready-made forms to start from (new forms only).
	const fld = (label: string, kind: string, maps_to: string, required = false, options: string[] = [], placeholder = ''): Field => ({ key: '', label, kind, required, maps_to, options, placeholder });
	const TEMPLATES: Record<string, { label: string; name: string; fields: Field[]; settings: { title: string; description: string; button: string; success_message: string } }> = {
		contact: { label: 'Contact us', name: 'Contact us', fields: [fld('Name', 'text', 'first_name', true), fld('Email', 'email', 'email', true, [], 'you@example.com'), fld('Phone', 'phone', 'phone'), fld('Message', 'textarea', 'comments')], settings: { title: 'Get in touch', description: 'Leave your details and we will get back to you within one working day.', button: 'Send', success_message: 'Thank you! We will be in touch soon.' } },
		callback: { label: 'Call me back', name: 'Call back', fields: [fld('Name', 'text', 'first_name', true), fld('Phone', 'phone', 'phone', true), fld('Best time to call', 'select', '', false, ['Morning', 'Afternoon', 'Evening'])], settings: { title: 'We’ll call you', description: 'Leave your number and we call you back.', button: 'Call me back', success_message: 'Thank you! We’ll call you soon.' } },
		quote: { label: 'Ask for a quote', name: 'Quote request', fields: [fld('Name', 'text', 'first_name', true), fld('Company', 'text', 'company_name'), fld('Email', 'email', 'email', true), fld('Phone', 'phone', 'phone'), fld('What do you need?', 'textarea', 'comments', true), fld('Budget', 'select', '', false, ['Under 1,000', '1,000–10,000', 'Over 10,000'])], settings: { title: 'Ask for a quote', description: 'Tell us what you need; we’ll send an offer.', button: 'Send request', success_message: 'Thank you! Your quote is on its way.' } },
		event: { label: 'Event registration', name: 'Event registration', fields: [fld('First name', 'text', 'first_name', true), fld('Last name', 'text', 'last_name', true), fld('Email', 'email', 'email', true), fld('Company', 'text', 'company_name'), fld('I’ll attend', 'select', '', true, ['In person', 'Online'])], settings: { title: 'Register', description: 'Save your seat.', button: 'Register', success_message: 'You’re registered. See you there!' } },
		newsletter: { label: 'Newsletter sign-up', name: 'Newsletter', fields: [fld('Email', 'email', 'email', true), fld('First name', 'text', 'first_name')], settings: { title: 'Stay in the loop', description: 'News and offers, now and then.', button: 'Sign up', success_message: 'Thanks for signing up!' } },
		feedback: { label: 'Feedback', name: 'Feedback', fields: [fld('Name', 'text', 'first_name'), fld('Email', 'email', 'email'), fld('How did we do?', 'select', '', true, ['Great', 'Good', 'Could be better', 'Poor']), fld('Comments', 'textarea', 'comments')], settings: { title: 'How did we do?', description: 'Your feedback helps us improve.', button: 'Send', success_message: 'Thank you for your feedback!' } }
	};
	let template = $state('contact');
	function useTemplate(k: string) {
		const t = TEMPLATES[k];
		template = k;
		draft!.name = t.name;
		draft!.fields = structuredClone(t.fields);
		draft!.settings = { ...draft!.settings, ...t.settings };
	}

	async function changeType(et: number) {
		draft!.entity_type = et;
		draft!.category_id = null;
		draft!.stage_id = null;
		await loadTargets(et);
		// Mappings that do not exist for the new type are cleared.
		for (const f of draft!.fields) if (f.maps_to && !targets.some((t) => t.code === f.maps_to)) f.maps_to = '';
	}

	function addField() {
		draft!.fields.push({ key: '', label: 'New field', kind: 'text', required: false, maps_to: '', options: [], placeholder: '' });
	}
	function move(i: number, d: -1 | 1) {
		const f = draft!.fields;
		[f[i], f[i + d]] = [f[i + d]!, f[i]!];
	}

	async function save() {
		saving = true;
		try {
			const d = draft!;
			const body = {
				name: d.name, entity_type: d.entity_type, category_id: d.category_id, stage_id: d.stage_id, is_active: d.is_active,
				fields: d.fields.map((f) => ({ ...f, options: f.kind === 'select' ? f.options : [] })),
				settings: d.settings, responsible: d.people.map((p) => p.id),
				reply_mailbox: d.reply_mailbox, reply_subject: d.reply_subject, reply_body: d.reply_body
			};
			const f = d.id ? await api.patch<Form>(`/crm/webforms/${d.id}`, body) : await api.post<Form>('/crm/webforms', body);
			await load();
			await open(f.id);
			toasts.success('Form saved');
		} catch (e) {
			toasts.error(e);
		} finally {
			saving = false;
		}
	}

	async function remove() {
		if (!draft?.id || !confirm(`Delete the form “${draft.name}”? Records it created stay.`)) return;
		await api.del(`/crm/webforms/${draft.id}`);
		draft = null;
		await load();
	}

	async function loadResults() {
		results = (await api.get<{ items: typeof results }>(`/crm/webforms/${draft!.id}/submissions`)).items;
	}
	$effect(() => {
		if (tab === 'results' && draft?.id) loadResults();
	});

	const preview = $derived<PublicForm | null>(
		draft ? { name: draft.name, company: '', settings: draft.settings, fields: draft.fields.map((f, i) => ({ ...f, key: f.key || `p${i}` })), token: '' } : null
	);
	const origin = $derived(base.replace(/\/f\/$/, ''));
	const link = $derived(draft?.key ? `${base}${draft.key}` : '');
	const scriptCode = $derived(draft?.key ? `<script src="${origin}/embed.js" data-form="${draft.key}" async><\/script>` : '');
	const iframeCode = $derived(draft?.key ? `<iframe src="${link}?embed=1" style="width:100%;max-width:560px;height:640px;border:0" title="${draft.name}"></iframe>` : '');
	function copy(text: string) {
		navigator.clipboard?.writeText(text).then(() => toasts.success('Copied'));
	}
</script>

<svelte:head><title>Web forms · NERP</title></svelte:head>

<div class="space-y-5">
	<div class="flex items-start justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-slate-900">Web forms</h1>
			<p class="text-sm text-slate-500">Forms for your website. Each submission becomes a lead (or a deal with its contact), with the page and campaign it came from.</p>
		</div>
		{#if crm.config?.is_admin}<button class="btn btn-primary shrink-0" onclick={() => open(null)}><Icon name="plus" class="size-4" /> New form</button>{/if}
	</div>

	{#if crm.config && !crm.config.is_admin}
		<p class="card p-6 text-sm text-slate-600">You need the “Configure CRM” permission to manage web forms.</p>
	{:else}
		<div class="grid gap-5 lg:grid-cols-[18rem_minmax(0,1fr)]">
			<section class="card self-start">
				<ul class="divide-y divide-slate-50">
					{#each items as f (f.id)}
						<li>
							<button class="w-full px-4 py-3 text-left text-sm hover:bg-slate-50 {draft?.id === f.id ? 'bg-brand-50' : ''}" onclick={() => open(f.id)}>
								<span class="block truncate font-medium {f.is_active ? 'text-slate-900' : 'text-slate-400'}">{f.name}</span>
								<span class="block text-xs text-slate-500">{f.submissions} submission{f.submissions === 1 ? '' : 's'}{f.last_submission_at ? ` · last ${formatRelative(f.last_submission_at)}` : ''}{f.is_active ? '' : ' · off'}</span>
							</button>
						</li>
					{:else}
						<li class="px-4 py-6 text-sm text-slate-500">No forms yet.</li>
					{/each}
				</ul>
			</section>

			{#if draft}
				<div class="min-w-0 space-y-4">
					<nav class="flex gap-1">
						{#each [['build', 'Build'], ['share', 'Share & embed'], ['results', 'Submissions']] as [k, l] (k)}
							<button class="btn {tab === k ? 'bg-brand-100 text-brand-900' : 'btn-ghost'}" disabled={k !== 'build' && !draft.id} onclick={() => (tab = k as typeof tab)}>{l}</button>
						{/each}
					</nav>

					{#if tab === 'build'}
						<div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_26rem]">
							<div class="space-y-4">
								<section class="card space-y-3 p-5 text-sm">
									<div class="grid gap-3 sm:grid-cols-2">
										{#if !draft.id}
											<div><label class="label" for="wf-tpl">Start from</label><select id="wf-tpl" class="input" value={template} onchange={(e) => useTemplate(e.currentTarget.value)}>{#each Object.entries(TEMPLATES) as [k, t] (k)}<option value={k}>{t.label}</option>{/each}</select></div>
										{/if}
										<div><label class="label" for="wf-name">Form name (internal)</label><input id="wf-name" class="input" maxlength="200" bind:value={draft.name} /></div>
										<div>
											<label class="label" for="wf-type">Creates</label>
											<select id="wf-type" class="input" value={draft.entity_type} onchange={(e) => changeType(Number(e.currentTarget.value))}>
												{#each types as t (t.id)}<option value={t.id}>{t.code === 'lead' ? 'a lead' : `a ${t.name.toLowerCase()} with its contact`}</option>{/each}
											</select>
										</div>
										{#if type && type.categories.length > 1}
											<div>
												<label class="label" for="wf-cat">Pipeline</label>
												<select id="wf-cat" class="input" value={draft.category_id ?? ''} onchange={(e) => ((draft!.category_id = e.currentTarget.value || null), (draft!.stage_id = null))}>
													<option value="">Default</option>
													{#each type.categories as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
												</select>
											</div>
										{/if}
										{#if category}
											<div>
												<label class="label" for="wf-stage">Stage</label>
												<select id="wf-stage" class="input" value={draft.stage_id ?? ''} onchange={(e) => (draft!.stage_id = e.currentTarget.value || null)}>
													<option value="">First stage</option>
													{#each category.stages.filter((s) => s.semantics === 'process') as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
												</select>
											</div>
										{/if}
									</div>
									<div>
										<span class="label">Responsible <span class="font-normal text-slate-400">(new records are assigned in turn; empty = you)</span></span>
										<MultiUserPicker bind:value={draft.people} />
									</div>
									<label class="flex items-center gap-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.is_active} /> Accepting submissions</label>
								</section>

								<section class="card">
									<h2 class="border-b border-slate-100 px-5 py-3 font-medium text-slate-900">Fields</h2>
									<ul class="divide-y divide-slate-50">
										{#each draft.fields as f, i (i)}
											<li class="space-y-2 px-5 py-3 text-sm">
												<div class="flex items-center gap-2">
													<input class="input min-w-0 flex-1" aria-label="Label" bind:value={f.label} />
													<button class="text-slate-400 hover:text-slate-800 disabled:opacity-20" disabled={i === 0} aria-label="Up" onclick={() => move(i, -1)}><Icon name="chevronDown" class="size-4 rotate-180" /></button>
													<button class="text-slate-400 hover:text-slate-800 disabled:opacity-20" disabled={i === draft.fields.length - 1} aria-label="Down" onclick={() => move(i, 1)}><Icon name="chevronDown" class="size-4" /></button>
													<button class="text-slate-300 hover:text-red-600" aria-label="Remove field" onclick={() => draft!.fields.splice(i, 1)}><Icon name="x" class="size-4" /></button>
												</div>
												<div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)_auto] items-center gap-2">
													<select class="input" aria-label="Type" bind:value={f.kind}>{#each KINDS as [k, l] (k)}<option value={k}>{l}</option>{/each}</select>
													<select class="input" aria-label="Saved into" title="Where the answer is saved" bind:value={f.maps_to}>
														<option value="">→ Comment</option>
														{#each targets as t (t.code)}<option value={t.code}>→ {t.label}</option>{/each}
													</select>
													<label class="flex items-center gap-1 text-xs whitespace-nowrap"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={f.required} /> Required</label>
												</div>
												{#if f.kind === 'select'}
													<textarea class="input" rows="2" placeholder="One option per line" aria-label="Options" value={f.options.join('\n')} oninput={(e) => (f.options = e.currentTarget.value.split('\n'))}></textarea>
												{:else if !['checkbox', 'date'].includes(f.kind)}
													<input class="input text-xs" placeholder="Placeholder (optional)" aria-label="Placeholder" bind:value={f.placeholder} />
												{/if}
											</li>
										{/each}
									</ul>
									<div class="border-t border-slate-100 px-5 py-3"><button class="btn btn-ghost text-sm" onclick={addField}><Icon name="plus" class="size-4" /> Add field</button></div>
								</section>

								<section class="card space-y-3 p-5 text-sm">
									<h2 class="font-medium text-slate-900">Texts & look</h2>
									<div class="grid gap-3 sm:grid-cols-2">
										<div><label class="label" for="wf-title">Heading</label><input id="wf-title" class="input" bind:value={draft.settings.title} /></div>
										<div><label class="label" for="wf-button">Button</label><input id="wf-button" class="input" maxlength="60" bind:value={draft.settings.button} /></div>
										<div class="sm:col-span-2"><label class="label" for="wf-desc">Introduction</label><textarea id="wf-desc" class="input" rows="2" bind:value={draft.settings.description}></textarea></div>
										<div class="sm:col-span-2"><label class="label" for="wf-done">Thank-you message</label><input id="wf-done" class="input" bind:value={draft.settings.success_message} /></div>
										<div><label class="label" for="wf-redirect">Or go to page after sending</label><input id="wf-redirect" class="input" placeholder="https://…" bind:value={draft.settings.redirect_url} /></div>
										<div><label class="label" for="wf-color">Colour</label><input id="wf-color" class="input h-10 p-1" type="color" value={draft.settings.color || '#0b57d0'} oninput={(e) => (draft!.settings.color = e.currentTarget.value)} /></div>
										<div class="sm:col-span-2"><label class="label" for="wf-consent">Consent checkbox text <span class="font-normal text-slate-400">(optional; must be ticked to send)</span></label><textarea id="wf-consent" class="input" rows="2" placeholder="I agree that my data is processed to answer my request." bind:value={draft.settings.consent_text}></textarea></div>
									</div>
								</section>

								<section class="card space-y-3 p-5 text-sm">
									<h2 class="font-medium text-slate-900">Automatic reply</h2>
									{#if mailboxes.length}
										<select class="input" aria-label="Reply from mailbox" value={draft.reply_mailbox ?? ''} onchange={(e) => (draft!.reply_mailbox = e.currentTarget.value || null)}>
											<option value="">No automatic reply</option>
											{#each mailboxes as m (m.id)}<option value={m.id}>From {m.email}</option>{/each}
										</select>
										{#if draft.reply_mailbox}
											<input class="input" placeholder="Subject" aria-label="Reply subject" bind:value={draft.reply_subject} />
											<textarea class="input" rows="4" placeholder="Message" aria-label="Reply message" bind:value={draft.reply_body}></textarea>
											<p class="text-xs text-slate-500">Sent to the form's email field. Use {'{{form}}'} and the answers: {draft.fields.filter((f) => f.key).map((f) => `{{${f.key}}} = ${f.label}`).join(', ') || 'save the form first to see field codes'}.</p>
										{/if}
									{:else}
										<p class="text-slate-500">Connect a mailbox under <a class="text-brand-600 hover:underline" href="/crm/mailboxes">Mailboxes</a> to send automatic replies.</p>
									{/if}
								</section>

								<div class="flex gap-2">
									<button class="btn btn-primary" disabled={saving || !draft.name.trim() || !draft.fields.length} onclick={save}>{saving ? 'Saving…' : draft.id ? 'Save form' : 'Create form'}</button>
									{#if draft.id}<button class="btn btn-ghost ml-auto text-red-600" onclick={remove}><Icon name="trash" class="size-4" /> Delete</button>{/if}
								</div>
							</div>

							<div class="xl:sticky xl:top-4 xl:self-start">
								<p class="mb-2 text-xs font-medium tracking-wide text-slate-500 uppercase">Preview</p>
								<div class="rounded-2xl bg-slate-100 p-4">{#if preview}<WebFormView form={preview} preview />{/if}</div>
							</div>
						</div>
					{:else if tab === 'share'}
						<section class="card space-y-4 p-5 text-sm">
							<div>
								<p class="label">Link</p>
								<div class="flex gap-2"><input class="input font-mono text-xs" readonly value={link} /><button class="btn btn-secondary" onclick={() => copy(link)}>Copy</button><a class="btn btn-ghost" href={link} target="_blank" rel="noopener">Open</a></div>
							</div>
							<div>
								<p class="label">Embed on your website <span class="font-normal text-slate-400">(recommended: resizes itself and records the page's UTM tags)</span></p>
								<div class="flex gap-2"><textarea class="input font-mono text-xs" rows="2" readonly value={scriptCode}></textarea><button class="btn btn-secondary self-start" onclick={() => copy(scriptCode)}>Copy</button></div>
							</div>
							<div>
								<p class="label">Or as a plain iframe</p>
								<div class="flex gap-2"><textarea class="input font-mono text-xs" rows="2" readonly value={iframeCode}></textarea><button class="btn btn-secondary self-start" onclick={() => copy(iframeCode)}>Copy</button></div>
							</div>
							<p class="text-xs text-slate-500">Links with <code>utm_source</code>, <code>utm_medium</code>, <code>utm_campaign</code>, <code>utm_content</code> or <code>utm_term</code> are saved on the created record.</p>
						</section>
					{:else}
						<section class="card">
							<ul class="divide-y divide-slate-50">
								{#each results as r (r.id)}
									<li class="px-5 py-3 text-sm">
										<div class="flex items-center gap-2">
											{#if r.item_id && r.entity_type}<a class="font-medium text-brand-700 hover:underline" href={itemLink(r.entity_type, r.item_id)}>{r.item_title}</a>{:else}<span class="text-slate-400">record deleted</span>{/if}
											<span class="ml-auto text-xs text-slate-500">{formatRelative(r.created_at)}</span>
										</div>
										<p class="mt-1 text-xs text-slate-600">{r.data.map((d) => `${d.label}: ${d.value}`).join(' · ')}</p>
										{#if r.utm || r.page_url}<p class="mt-0.5 text-xs text-slate-400">{r.page_url ?? ''}{r.utm ? ` · ${Object.entries(r.utm).map(([k, v]) => `${k}=${v}`).join(' ')}` : ''}</p>{/if}
									</li>
								{:else}
									<li class="px-5 py-6 text-sm text-slate-500">No submissions yet.</li>
								{/each}
							</ul>
						</section>
					{/if}
				</div>
			{:else}
				<div class="card p-10 text-center text-sm text-slate-500">Pick a form or create a new one.</div>
			{/if}
		</div>
	{/if}
</div>
