<script lang="ts">
	// Contact centre → Chat buttons: the website chat buttons, who answers them, and the code
	// to put on the website.
	import { onMount } from 'svelte';
	import { api } from '#lib/api.ts';
	import Icon from '#lib/components/Icon.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import MultiUserPicker from '#lib/components/MultiUserPicker.svelte';
	import { toasts } from '#lib/toast.svelte.ts';

	type Person = { id: string; name: string; avatar_file_id: string | null };
	type Channel = {
		id: string; name: string; key: string; title: string; greeting: string; color: string; ask_details: boolean; operators: string[]; is_active: boolean; open_chats: number;
		distribution: 'queue' | 'all'; max_chats: number | null; requeue_minutes: number | null; hours: [number, number, number][] | null; timezone: string;
		offline_message: string | null; auto_reply: string | null; no_answer_minutes: number | null; no_answer_message: string | null; close_message: string | null;
		idle_close_minutes: number | null; consent_text: string | null; ask_rating: boolean;
	};
	type Canned = { id: string; channel_id: string | null; shortcut: string; title: string; body: string };
	const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
	const TZ = Intl.DateTimeFormat().resolvedOptions().timeZone;
	const hm = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
	const mins = (s: string) => { const [h, m] = s.split(':').map(Number); return h * 60 + m; };
	const num = (v: unknown) => (v === '' || v === null || v === undefined ? null : Number(v));
	let items = $state<Channel[]>([]);
	let people = $state<Person[]>([]);
	let base = $state('');
	let canManage = $state(false);
	function take(r: { items: Channel[]; people: Person[]; public_base: string; can_manage: boolean }) {
		items = r.items;
		people = r.people;
		base = r.public_base;
		canManage = r.can_manage;
	}
	onMount(async () => take(await api.get('/chat/channels')));

	let open = $state(false);
	let editing = $state<Channel | null>(null);
	let form = $state(blank());
	function blank() {
		return {
			name: '', title: 'Chat with us', greeting: 'Hi! How can we help?', color: '#1a73e8', ask_details: true, is_active: true,
			distribution: 'queue', max_chats: '' as string | number, requeue_minutes: '' as string | number, always: true, timezone: TZ,
			days: DAYS.map((_, i) => ({ on: i < 5, from: '09:00', to: '18:00' })), offline_message: '', auto_reply: '', no_answer_minutes: '' as string | number,
			no_answer_message: '', close_message: '', idle_close_minutes: '' as string | number, consent_text: '', ask_rating: true
		};
	}
	let ops = $state<Person[]>([]);
	function edit(c: Channel | null) {
		editing = c;
		form = c
			? {
					name: c.name, title: c.title, greeting: c.greeting, color: c.color, ask_details: c.ask_details, is_active: c.is_active,
					distribution: c.distribution, max_chats: c.max_chats ?? '', requeue_minutes: c.requeue_minutes ?? '', always: !c.hours, timezone: c.timezone,
					days: DAYS.map((_, i) => { const h = c.hours?.find((x) => x[0] === i + 1); return { on: !!h, from: h ? hm(h[1]) : '09:00', to: h ? hm(h[2]) : '18:00' }; }),
					offline_message: c.offline_message ?? '', auto_reply: c.auto_reply ?? '', no_answer_minutes: c.no_answer_minutes ?? '', no_answer_message: c.no_answer_message ?? '',
					close_message: c.close_message ?? '', idle_close_minutes: c.idle_close_minutes ?? '', consent_text: c.consent_text ?? '', ask_rating: c.ask_rating
				}
			: blank();
		ops = c ? c.operators.map((id) => people.find((p) => p.id === id)).filter((p): p is Person => !!p) : [];
		open = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			const f = form;
			const body = {
				name: f.name, title: f.title, greeting: f.greeting, color: f.color, ask_details: f.ask_details, is_active: f.is_active, operators: ops.map((o) => o.id),
				distribution: f.distribution, max_chats: num(f.max_chats), requeue_minutes: num(f.requeue_minutes), timezone: f.timezone,
				hours: f.always ? null : f.days.map((d, i) => [i + 1, mins(d.from), d.to === '00:00' ? 1440 : mins(d.to), d.on] as const).filter((x) => x[3]).map(([d, a, b]) => [d, a, b]),
				offline_message: f.offline_message || null, auto_reply: f.auto_reply || null, no_answer_minutes: num(f.no_answer_minutes), no_answer_message: f.no_answer_message || null,
				close_message: f.close_message || null, idle_close_minutes: num(f.idle_close_minutes), consent_text: f.consent_text || null, ask_rating: f.ask_rating
			};
			take(editing ? await api.patch(`/chat/channels/${editing.id}`, body) : await api.post('/chat/channels', body));
			open = false;
		} catch (err) {
			toasts.error(err);
		}
	}
	async function remove(c: Channel) {
		if (!confirm(`Delete “${c.name}” and all its chats? Transcripts already on CRM records stay.`)) return;
		take(await api.del(`/chat/channels/${c.id}`));
	}
	// Saved replies.
	let canned = $state<Canned[]>([]);
	let cannedForm = $state({ shortcut: '', title: '', body: '', channel_id: '' });
	onMount(async () => {
		canned = (await api.get<{ items: Canned[] }>('/chat/canned')).items;
	});
	async function addCanned(e: SubmitEvent) {
		e.preventDefault();
		try {
			canned = (await api.post<{ items: Canned[] }>('/chat/canned', { ...cannedForm, channel_id: cannedForm.channel_id || null })).items;
			cannedForm = { shortcut: '', title: '', body: '', channel_id: '' };
		} catch (err) {
			toasts.error(err);
		}
	}
	async function dropCanned(c: Canned) {
		canned = (await api.del<{ items: Canned[] }>(`/chat/canned/${c.id}`)).items;
	}
	const code = (c: Channel) => `<script src="${base}/chat.js" data-chat="${c.key}" data-color="${c.color}" async><\/script>`;
	async function copy(c: Channel) {
		await navigator.clipboard.writeText(code(c));
		toasts.success('Copied');
	}
</script>

<svelte:head><title>Chat buttons · NERP</title></svelte:head>

<div class="max-w-4xl space-y-4">
	<div class="flex items-center gap-3">
		<div class="flex-1">
			<h1 class="text-2xl font-semibold text-slate-900">Chat buttons</h1>
			<p class="mt-1 text-sm text-slate-500">A chat button on your website. Chats go to its operators in turn and appear under <a class="text-brand-600 hover:underline" href="/chats">Chats</a>; each is linked to a lead or contact.</p>
		</div>
		{#if canManage}<button class="btn btn-primary" onclick={() => edit(null)}><Icon name="plus" class="size-4" /> New chat button</button>{/if}
	</div>
	{#each items as c (c.id)}
		<section class="card space-y-3 p-5" data-channel={c.name}>
			<div class="flex flex-wrap items-center gap-3">
				<span class="size-4 rounded-full" style="background:{c.color}"></span>
				<h2 class="flex-1 font-medium text-slate-900">{c.name}{#if !c.is_active}<span class="badge ml-2 bg-slate-100">Off</span>{/if}</h2>
				<span class="text-sm text-slate-500">{c.open_chats} open</span>
				<a class="btn btn-ghost px-2 py-1 text-sm" href="/c/{c.key}" target="_blank" rel="noopener">Try it</a>
				{#if canManage}
					<button class="btn btn-ghost p-1.5" aria-label="Edit {c.name}" onclick={() => edit(c)}><Icon name="edit" class="size-4" /></button>
					<button class="btn btn-ghost p-1.5 hover:text-red-600" aria-label="Delete {c.name}" onclick={() => remove(c)}><Icon name="trash" class="size-4" /></button>
				{/if}
			</div>
			<p class="text-sm text-slate-600">Operators: {c.operators.map((id) => people.find((p) => p.id === id)?.name).filter(Boolean).join(', ') || 'nobody (chats wait for anyone who opens them)'}</p>
			<div>
				<p class="mb-1 text-xs font-medium text-slate-500">Put this on every page of your website, before &lt;/body&gt;:</p>
				<div class="flex gap-2">
					<code class="block flex-1 overflow-x-auto rounded-lg bg-slate-50 px-3 py-2 text-xs whitespace-nowrap text-slate-700" data-code>{code(c)}</code>
					<button class="btn btn-secondary" onclick={() => copy(c)}>Copy</button>
				</div>
			</div>
		</section>
	{:else}
		<div class="card p-10 text-center text-slate-500">No chat buttons yet.</div>
	{/each}
	<section class="card space-y-3 p-5" data-canned-admin>
		<h2 class="font-medium text-slate-900">Saved replies</h2>
		<p class="text-sm text-slate-500">Operators type <span class="font-mono">/</span> and the short code in a chat to use one.</p>
		{#if canned.length}
			<ul class="divide-y divide-slate-100 text-sm">
				{#each canned as c (c.id)}
					<li class="flex items-start gap-3 py-2"><span class="font-mono text-brand-700">/{c.shortcut}</span><span class="flex-1"><span class="font-medium">{c.title}</span> <span class="text-slate-500">{c.channel_id ? `· ${items.find((x) => x.id === c.channel_id)?.name ?? ''}` : '· all chat buttons'}</span><br /><span class="text-slate-600">{c.body}</span></span>{#if canManage}<button class="text-slate-400 hover:text-red-600" aria-label="Delete /{c.shortcut}" onclick={() => dropCanned(c)}>×</button>{/if}</li>
				{/each}
			</ul>
		{/if}
		{#if canManage}
			<form class="grid gap-2 sm:grid-cols-4" onsubmit={addCanned}>
				<input class="input" aria-label="Short code" placeholder="Short code, e.g. hours" required maxlength="30" bind:value={cannedForm.shortcut} />
				<input class="input" aria-label="Title" placeholder="Title" required maxlength="120" bind:value={cannedForm.title} />
				<select class="input" aria-label="For" bind:value={cannedForm.channel_id}><option value="">All chat buttons</option>{#each items as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</select>
				<button class="btn btn-secondary">Add</button>
				<textarea class="input sm:col-span-4" rows="2" aria-label="Reply text" placeholder="The reply" required maxlength="4000" bind:value={cannedForm.body}></textarea>
			</form>
		{/if}
	</section>
</div>

<Modal bind:open title={editing ? 'Edit chat button' : 'New chat button'}>
	<form id="ch-form" class="grid gap-3 sm:grid-cols-2" onsubmit={save}>
		<div class="sm:col-span-2"><label class="label" for="cf-name">Name (for you)</label><input id="cf-name" class="input" required maxlength="120" placeholder="Website" bind:value={form.name} /></div>
		<div><label class="label" for="cf-title">Title (for visitors)</label><input id="cf-title" class="input" required maxlength="80" bind:value={form.title} /></div>
		<div><label class="label" for="cf-color">Colour</label><input id="cf-color" class="input h-10 p-1" type="color" bind:value={form.color} /></div>
		<div class="sm:col-span-2"><label class="label" for="cf-greet">Greeting</label><textarea id="cf-greet" class="input" rows="2" required maxlength="500" bind:value={form.greeting}></textarea></div>
		<div class="sm:col-span-2"><span class="label">Operators</span><MultiUserPicker bind:value={ops} placeholder="Who answers…" /></div>
		<label class="flex items-center gap-2 text-sm sm:col-span-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.ask_details} /> Ask for the visitor's name and email first</label>
		{#if editing}<label class="flex items-center gap-2 text-sm sm:col-span-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.is_active} /> On</label>{/if}
		<h3 class="mt-2 font-medium text-slate-800 sm:col-span-2">Handing chats out</h3>
		<div><label class="label" for="cf-dist">New chats go</label><select id="cf-dist" class="input" bind:value={form.distribution}><option value="queue">To each operator in turn</option><option value="all">To everyone (first to take it)</option></select></div>
		<div><label class="label" for="cf-max">At most open chats each <span class="font-normal text-slate-400">(optional)</span></label><input id="cf-max" class="input" type="number" min="1" max="100" bind:value={form.max_chats} /></div>
		<div class="sm:col-span-2"><label class="label" for="cf-requeue">Pass to the next operator after (minutes without an answer) <span class="font-normal text-slate-400">(optional)</span></label><input id="cf-requeue" class="input w-32" type="number" min="1" max="240" bind:value={form.requeue_minutes} /></div>
		<h3 class="mt-2 font-medium text-slate-800 sm:col-span-2">Working hours</h3>
		<label class="flex items-center gap-2 text-sm sm:col-span-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.always} /> Always open</label>
		{#if !form.always}
			<fieldset class="space-y-1.5 text-sm sm:col-span-2">
				<legend class="sr-only">Hours ({form.timezone})</legend>
				{#each form.days as d, i (i)}
					<div class="flex items-center gap-2"><label class="flex w-16 items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.days[i].on} /> {DAYS[i]}</label>{#if d.on}<input class="input w-28 py-1" type="time" aria-label="{DAYS[i]} from" bind:value={form.days[i].from} />–<input class="input w-28 py-1" type="time" aria-label="{DAYS[i]} to" bind:value={form.days[i].to} />{:else}<span class="text-slate-400">Closed</span>{/if}</div>
				{/each}
				<p class="text-xs text-slate-500">In {form.timezone.replace(/_/g, ' ')}.</p>
			</fieldset>
			<div class="sm:col-span-2"><label class="label" for="cf-off">Away message</label><textarea id="cf-off" class="input" rows="2" maxlength="2000" placeholder="We're away now; leave a message and we'll reply tomorrow." bind:value={form.offline_message}></textarea></div>
		{/if}
		<h3 class="mt-2 font-medium text-slate-800 sm:col-span-2">Automatic messages</h3>
		<div class="sm:col-span-2"><label class="label" for="cf-auto">After the visitor's first message</label><textarea id="cf-auto" class="input" rows="2" maxlength="2000" bind:value={form.auto_reply}></textarea></div>
		<div><label class="label" for="cf-nam">When nobody answers within (minutes)</label><input id="cf-nam" class="input" type="number" min="1" max="240" bind:value={form.no_answer_minutes} /></div>
		<div><label class="label" for="cf-nat">…say</label><input id="cf-nat" class="input" maxlength="2000" bind:value={form.no_answer_message} /></div>
		<div><label class="label" for="cf-idle">Close chats idle for (minutes)</label><input id="cf-idle" class="input" type="number" min="5" max="10080" bind:value={form.idle_close_minutes} /></div>
		<div><label class="label" for="cf-bye">Goodbye on closing</label><input id="cf-bye" class="input" maxlength="2000" bind:value={form.close_message} /></div>
		<h3 class="mt-2 font-medium text-slate-800 sm:col-span-2">Consent and rating</h3>
		<div class="sm:col-span-2"><label class="label" for="cf-consent">Visitors agree to this before chatting <span class="font-normal text-slate-400">(optional)</span></label><textarea id="cf-consent" class="input" rows="2" maxlength="2000" placeholder="I agree that my messages are stored as described in the privacy notice." bind:value={form.consent_text}></textarea></div>
		<label class="flex items-center gap-2 text-sm sm:col-span-2"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={form.ask_rating} /> Ask visitors to rate the chat</label>
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="ch-form">Save</button>
	{/snippet}
</Modal>
