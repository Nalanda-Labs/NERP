<script lang="ts">
	// The visual (what-you-see-is-what-you-get) editor for knowledge base pages, shared live
	// with everyone editing the page at the same time: their changes appear as they type, with
	// their cursors and names. The page is still kept as Markdown.
	import { onDestroy, onMount } from 'svelte';
	import { Editor, Extension } from '@tiptap/core';
	import StarterKit from '@tiptap/starter-kit';
	import { Markdown } from '@tiptap/markdown';
	import { TableKit } from '@tiptap/extension-table';
	import { TaskItem, TaskList } from '@tiptap/extension-list';
	import Image from '@tiptap/extension-image';
	import Collaboration from '@tiptap/extension-collaboration';
	import CollaborationCaret from '@tiptap/extension-collaboration-caret';
	import { KbCollab, type Person } from '#lib/kbCollab.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	let {
		pageId,
		markdown,
		upload,
		onchange,
		onreset,
		class: klass = ''
	}: {
		pageId: string;
		/** The saved text, put into a new live document. */
		markdown: string;
		upload?: (f: File) => Promise<{ url: string; name: string; mime_type?: string }>;
		onchange?: (md: string) => void;
		/** The page was changed outside the live document. */
		onreset?: () => void;
		class?: string;
	} = $props();

	let host: HTMLDivElement | undefined = $state();
	let editor = $state<Editor | null>(null);
	let collab: KbCollab | null = null;
	let people = $state<Person[]>([]);
	let ready = $state(false);
	let tick = $state(0);
	const COLORS = ['#e8590c', '#1971c2', '#2f9e44', '#9c36b5', '#c2255c', '#0c8599', '#e67700'];
	const myColor = COLORS[Math.abs([...(session.user?.id ?? 'x')].reduce((a, c) => a * 31 + c.charCodeAt(0), 7)) % COLORS.length]!;

	/** The page as Markdown now. */
	export function getMarkdown(): string {
		return editor?.getMarkdown() ?? markdown;
	}

	// On a line holding a colleague's cursor (a non-editable marker at the end of the text),
	// the browser's End and Home move the selection past the marker, where the editor can't
	// place it; there they move through the editor instead.
	const CaretSafeKeys = Extension.create({
		name: 'caretSafeKeys',
		addKeyboardShortcuts() {
			const move = (toEnd: boolean) => () => {
				const ed = this.editor;
				const { $from: from, empty } = ed.state.selection;
				if (!empty || !from.parent.isTextblock) return false;
				const at = ed.view.domAtPos(from.start()).node;
				const block = (at.nodeType === 1 ? (at as HTMLElement) : at.parentElement)?.closest('p, h1, h2, h3, h4, h5, h6, li, td, th, pre');
				if (!block?.querySelector('.collaboration-carets__caret')) return false;
				return ed.commands.setTextSelection(toEnd ? from.end() : from.start());
			};
			return { End: move(true), Home: move(false) };
		}
	});

	let changeTimer: ReturnType<typeof setTimeout>;
	onMount(async () => {
		const name = session.user ? `${session.user.first_name} ${session.user.last_name}` : 'Someone';
		collab = new KbCollab(pageId, { name, color: myColor }, { people: (p) => (people = p), reset: () => onreset?.(), error: () => {} });
		let seed = false;
		try {
			seed = await collab.start();
		} catch (e) {
			toasts.error(e);
		}
		editor = new Editor({
			element: host!,
			extensions: [
				StarterKit.configure({ undoRedo: false, link: { openOnClick: false } }),
				Markdown,
				TableKit.configure({ table: { resizable: false } }),
				TaskList,
				TaskItem.configure({ nested: true }),
				Image,
				Collaboration.configure({ document: collab.doc }),
				CollaborationCaret.configure({ provider: { awareness: collab.awareness }, user: { name, color: myColor } }),
				CaretSafeKeys
			],
			editorProps: { attributes: { class: 'prose-kb max-w-none min-h-[420px] px-6 py-4 focus:outline-none', 'aria-label': 'Page text', role: 'textbox', 'aria-multiline': 'true', 'data-visual-editor': '' } },
			onTransaction: () => (tick += 1),
			onUpdate: () => {
				clearTimeout(changeTimer);
				changeTimer = setTimeout(() => onchange?.(editor!.getMarkdown()), 250);
			}
		});
		// The first editor of a new live document puts the saved page in; others get it from them.
		if (seed && editor.isEmpty && markdown.trim()) editor.commands.setContent(markdown, { contentType: 'markdown' });
		ready = true;
	});
	onDestroy(() => {
		clearTimeout(changeTimer);
		editor?.destroy();
		collab?.destroy();
	});

	// `tick` makes the toolbar follow the selection (it changes with every transaction).
	const is = (name: string, attrs?: Record<string, unknown>) => tick >= 0 && (editor?.isActive(name, attrs) ?? false);
	function link() {
		const prev = editor!.getAttributes('link').href as string | undefined;
		const url = prompt('Link address (empty removes the link)', prev ?? 'https://');
		if (url === null) return;
		if (!url.trim()) editor!.chain().focus().unsetLink().run();
		else editor!.chain().focus().extendMarkRange('link').setLink({ href: url.trim() }).run();
	}
	async function picture(f: File | undefined) {
		if (!f || !upload) return;
		try {
			const u = await upload(f);
			if (f.type.startsWith('image/')) editor!.chain().focus().setImage({ src: u.url, alt: u.name }).run();
			else editor!.chain().focus().insertContent({ type: 'text', text: u.name, marks: [{ type: 'link', attrs: { href: u.url } }] }).run();
		} catch (e) {
			toasts.error(e);
		}
	}
	type Btn = { label: string; title: string; run: () => void; active?: () => boolean };
	const buttons: (Btn | null)[] = [
		{ label: 'B', title: 'Bold (Ctrl+B)', run: () => editor!.chain().focus().toggleBold().run(), active: () => is('bold') },
		{ label: 'I', title: 'Italic (Ctrl+I)', run: () => editor!.chain().focus().toggleItalic().run(), active: () => is('italic') },
		{ label: 'S', title: 'Strikethrough', run: () => editor!.chain().focus().toggleStrike().run(), active: () => is('strike') },
		{ label: '</>', title: 'Code', run: () => editor!.chain().focus().toggleCode().run(), active: () => is('code') },
		null,
		{ label: 'H1', title: 'Heading 1', run: () => editor!.chain().focus().toggleHeading({ level: 1 }).run(), active: () => is('heading', { level: 1 }) },
		{ label: 'H2', title: 'Heading 2', run: () => editor!.chain().focus().toggleHeading({ level: 2 }).run(), active: () => is('heading', { level: 2 }) },
		{ label: 'H3', title: 'Heading 3', run: () => editor!.chain().focus().toggleHeading({ level: 3 }).run(), active: () => is('heading', { level: 3 }) },
		null,
		{ label: '•', title: 'Bulleted list', run: () => editor!.chain().focus().toggleBulletList().run(), active: () => is('bulletList') },
		{ label: '1.', title: 'Numbered list', run: () => editor!.chain().focus().toggleOrderedList().run(), active: () => is('orderedList') },
		{ label: '☑', title: 'Checklist', run: () => editor!.chain().focus().toggleTaskList().run(), active: () => is('taskList') },
		{ label: '❝', title: 'Quote', run: () => editor!.chain().focus().toggleBlockquote().run(), active: () => is('blockquote') },
		{ label: '{ }', title: 'Code block', run: () => editor!.chain().focus().toggleCodeBlock().run(), active: () => is('codeBlock') },
		null,
		{ label: 'Link', title: 'Link', run: link, active: () => is('link') },
		{ label: 'Table', title: 'Insert a table', run: () => editor!.chain().focus().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run() },
		{ label: '—', title: 'Divider', run: () => editor!.chain().focus().setHorizontalRule().run() }
	];
</script>

<div class="flex flex-col overflow-hidden rounded-xl border border-slate-200 bg-white {klass}" data-visual-editor-wrap>
	<div class="flex flex-wrap items-center gap-0.5 border-b border-slate-100 px-2 py-1.5" role="toolbar" aria-label="Formatting">
		{#each buttons as b, i (i)}
			{#if b}
				<button type="button" class="min-w-8 rounded-md px-2 py-1 text-sm {b.active?.() ? 'bg-brand-50 text-brand-700' : 'text-slate-600 hover:bg-slate-100'}" title={b.title} aria-label={b.title} aria-pressed={b.active ? b.active() : undefined} disabled={!ready} onclick={b.run}>{b.label}</button>
			{:else}
				<span class="mx-1 h-5 w-px bg-slate-200" aria-hidden="true"></span>
			{/if}
		{/each}
		{#if upload}
			<label class="cursor-pointer rounded-md px-2 py-1 text-sm text-slate-600 hover:bg-slate-100" title="Picture or file">
				Picture
				<input type="file" class="hidden" onchange={(e) => picture(e.currentTarget.files?.[0])} />
			</label>
		{/if}
		<div class="ml-auto flex items-center gap-1" aria-label="Editing now" data-co-editors>
			{#each people as p (p.client)}
				<span class="rounded-full px-2 py-0.5 text-xs font-medium text-white" style="background:{p.color}" title="{p.name} is editing">{p.name}</span>
			{/each}
		</div>
	</div>
	<div class="min-h-0 flex-1 overflow-y-auto" bind:this={host}></div>
</div>

<style>
	:global(.collaboration-carets__caret) {
		position: relative;
		margin-left: -1px;
		margin-right: -1px;
		border-left: 1px solid;
		border-right: 1px solid;
		word-break: normal;
		pointer-events: none;
	}
	:global(.collaboration-carets__label) {
		position: absolute;
		top: -1.4em;
		left: -1px;
		font-size: 11px;
		font-weight: 600;
		line-height: normal;
		white-space: nowrap;
		color: #fff;
		padding: 0.1rem 0.3rem;
		border-radius: 3px 3px 3px 0;
		user-select: none;
	}
	:global([data-visual-editor] table) {
		border-collapse: collapse;
	}
	:global([data-visual-editor] td),
	:global([data-visual-editor] th) {
		border: 1px solid #cbd5e1;
		padding: 0.25rem 0.5rem;
		min-width: 4rem;
	}
	:global([data-visual-editor] ul[data-type='taskList']) {
		list-style: none;
		padding-left: 0.25rem;
	}
	:global([data-visual-editor] ul[data-type='taskList'] li) {
		display: flex;
		gap: 0.5rem;
	}
	:global([data-visual-editor] img) {
		max-width: 100%;
	}
</style>
