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
	type Channel = { id: string; name: string; key: string; title: string; greeting: string; color: string; ask_details: boolean; operators: string[]; is_active: boolean; open_chats: number };
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
	let form = $state({ name: '', title: 'Chat with us', greeting: 'Hi! How can we help?', color: '#1a73e8', ask_details: true, is_active: true });
	let ops = $state<Person[]>([]);
	function edit(c: Channel | null) {
		editing = c;
		form = c ? { name: c.name, title: c.title, greeting: c.greeting, color: c.color, ask_details: c.ask_details, is_active: c.is_active } : { name: '', title: 'Chat with us', greeting: 'Hi! How can we help?', color: '#1a73e8', ask_details: true, is_active: true };
		ops = c ? c.operators.map((id) => people.find((p) => p.id === id)).filter((p): p is Person => !!p) : [];
		open = true;
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		try {
			const body = { ...form, operators: ops.map((o) => o.id) };
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
	</form>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" form="ch-form">Save</button>
	{/snippet}
</Modal>
