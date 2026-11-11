<script lang="ts" module>
	export type LinkTarget = { id: string; title: string; href: string };
	export type Uploaded = { url: string; name: string; mime_type: string };
</script>

<script lang="ts">
	// A full Markdown editor: the source in CodeMirror (highlighted, with search, undo and
	// Markdown-aware Enter that continues lists), a toolbar and shortcuts for formatting,
	// pictures and files by button, paste or drag-and-drop, links to other pages (also by
	// typing [[), tables, callouts and footnotes, and a live preview beside it or instead
	// of it. The preview is rendered by the server, so it is exactly what readers will see.
	import { onDestroy, onMount, untrack } from 'svelte';
	import { EditorState, EditorSelection, type Extension } from '@codemirror/state';
	import { EditorView, keymap, placeholder as cmPlaceholder, drawSelection, dropCursor, highlightActiveLine } from '@codemirror/view';
	import { defaultKeymap, history, historyKeymap, indentWithTab, undo, redo } from '@codemirror/commands';
	import { markdown, markdownLanguage, markdownKeymap } from '@codemirror/lang-markdown';
	import { languages } from '@codemirror/language-data';
	import { syntaxHighlighting, HighlightStyle, indentOnInput, bracketMatching } from '@codemirror/language';
	import { search, searchKeymap, openSearchPanel, highlightSelectionMatches } from '@codemirror/search';
	import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap, type CompletionContext } from '@codemirror/autocomplete';
	import { tags as t } from '@lezer/highlight';
	import MarkdownView from './MarkdownView.svelte';
	import Icon, { type IconName } from './Icon.svelte';

	let {
		value = $bindable(''),
		render,
		upload,
		links = [],
		onsave,
		placeholder = '',
		label = 'Text (Markdown)',
		class: size = 'h-[70vh] min-h-[420px]'
	}: {
		value?: string;
		/** Markdown to the HTML readers see. */
		render: (md: string) => Promise<string>;
		/** Stores a picture or file and says where it is. */
		upload?: (file: File) => Promise<Uploaded>;
		/** Pages to link to (toolbar and [[ ). */
		links?: LinkTarget[];
		/** Ctrl+S. */
		onsave?: () => void;
		placeholder?: string;
		label?: string;
		/** Height classes (full screen ignores them). */
		class?: string;
	} = $props();

	type Mode = 'write' | 'split' | 'preview';
	let mode = $state<Mode>('split');
	let full = $state(false);
	let host: HTMLDivElement | undefined = $state();
	let previewEl: HTMLDivElement | undefined = $state();
	let view: EditorView | undefined;
	let html = $state('');
	let rendering = $state(false);
	let uploading = $state(0);
	let fileInput: HTMLInputElement | undefined = $state();
	let fileKind = $state<'image' | 'file'>('image');
	let linkOpen = $state(false);
	let linkQuery = $state('');
	let helpOpen = $state(false);
	const stats = $derived.by(() => {
		const words = value.trim() ? value.trim().split(/\s+/).length : 0;
		return { words, chars: value.length, minutes: Math.max(1, Math.round(words / 200)) };
	});

	// ---- preview ----
	let timer: ReturnType<typeof setTimeout> | undefined;
	function refresh(md: string) {
		clearTimeout(timer);
		timer = setTimeout(async () => {
			rendering = true;
			try {
				html = await render(md);
			} catch {
				/* keep the last preview */
			} finally {
				rendering = false;
			}
		}, 300);
	}

	// ---- editing helpers ----
	function focus() {
		view?.focus();
	}
	/** Wraps each selection in `before`/`after`, or unwraps it when it is wrapped already. */
	function wrap(before: string, after = before, empty = 'text') {
		if (!view) return;
		const { state } = view;
		view.dispatch(
			state.changeByRange((r) => {
				const text = state.sliceDoc(r.from, r.to);
				const outer = state.sliceDoc(r.from - before.length, r.to + after.length);
				if (outer.startsWith(before) && outer.endsWith(after) && r.from - before.length >= 0) {
					return { changes: [{ from: r.from - before.length, to: r.from, insert: '' }, { from: r.to, to: r.to + after.length, insert: '' }], range: EditorSelection.range(r.from - before.length, r.to - before.length) };
				}
				const inner = text || empty;
				return { changes: { from: r.from, to: r.to, insert: before + inner + after }, range: EditorSelection.range(r.from + before.length, r.from + before.length + inner.length) };
			})
		);
		focus();
	}
	/** Sets (or removes, when all lines have it) a prefix on every selected line. */
	function prefix(mark: string | ((i: number) => string), strip: RegExp) {
		if (!view) return;
		const { state } = view;
		const lines = new Set<number>();
		for (const r of state.selection.ranges) for (let n = state.doc.lineAt(r.from).number; n <= state.doc.lineAt(r.to).number; n++) lines.add(n);
		const list = [...lines].map((n) => state.doc.line(n));
		const mk = (i: number) => (typeof mark === 'string' ? mark : mark(i));
		const all = list.every((l, i) => l.text.startsWith(mk(i)));
		const changes = list.map((l, i) => {
			const existing = l.text.match(strip)?.[0] ?? '';
			return all ? { from: l.from, to: l.from + mk(i).length, insert: '' } : { from: l.from, to: l.from + existing.length, insert: mk(i) };
		});
		view.dispatch({ changes });
		focus();
	}
	const LINE_MARK = /^(#{1,6} |> |- \[[ x]\] |[-*+] |\d+\. )/;
	const heading = (n: number) => prefix('#'.repeat(n) + ' ', LINE_MARK);
	/** Inserts a block on lines of its own at the cursor. */
	function block(text: string, cursorAt?: number) {
		if (!view) return;
		const { state } = view;
		const pos = state.selection.main.head;
		// A block needs a blank line before it (or it joins the paragraph or list above).
		const above = state.sliceDoc(Math.max(0, pos - 2), pos);
		const before = pos === 0 ? '' : above.endsWith('\n\n') || (pos === 1 && above === '\n') ? '' : above.endsWith('\n') ? '\n' : '\n\n';
		const after = state.sliceDoc(pos, pos + 1);
		const insert = before + text + (after && after !== '\n' ? '\n\n' : '\n');
		view.dispatch({ changes: { from: pos, insert }, selection: { anchor: pos + before.length + (cursorAt ?? text.length) } });
		focus();
	}
	function insertAt(text: string) {
		if (!view) return;
		view.dispatch(view.state.replaceSelection(text));
		focus();
	}
	function link() {
		if (!view) return;
		const sel = view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to);
		const url = prompt('Link address', /^https?:\/\//.test(sel) ? sel : 'https://');
		if (!url) return;
		const text = /^https?:\/\//.test(sel) ? 'link' : sel || 'link';
		insertAt(`[${text}](${url.trim()})`);
	}
	const table = () => block('| Column | Column | Column |\n| --- | --- | --- |\n| Text | Text | Text |\n| Text | Text | Text |', 2);
	const codeBlock = () => {
		const sel = view ? view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to) : '';
		if (sel && view) {
			view.dispatch(view.state.replaceSelection('```\n' + sel + '\n```'));
			focus();
		} else block('```\n\n```', 4);
	};
	let footnotes = 0;
	function footnote() {
		if (!view) return;
		const used = [...view.state.doc.toString().matchAll(/\[\^(\d+)\]/g)].map((m) => Number(m[1]));
		const n = Math.max(footnotes, ...used, 0) + 1;
		footnotes = n;
		const end = view.state.doc.length;
		view.dispatch({ changes: [{ from: view.state.selection.main.head, insert: `[^${n}]` }, { from: end, insert: `${view.state.doc.toString().endsWith('\n') ? '' : '\n'}\n[^${n}]: ` }], selection: { anchor: end + `[^${n}]`.length + (view.state.doc.toString().endsWith('\n') ? 1 : 2) + `[^${n}]: `.length } });
		focus();
	}
	const CALLOUTS = [['NOTE', 'Note'], ['TIP', 'Tip'], ['IMPORTANT', 'Important'], ['WARNING', 'Warning'], ['CAUTION', 'Caution']] as const;
	let calloutOpen = $state(false);
	function callout(kind: string) {
		calloutOpen = false;
		block(`> [!${kind}]\n> `);
	}

	// ---- pictures and files ----
	async function put(files: File[], at?: number) {
		if (!upload || !view || !files.length) return;
		for (const f of files) {
			const image = f.type.startsWith('image/');
			const marker = `${image ? '!' : ''}[Uploading ${f.name}…]()`;
			const pos = at ?? view.state.selection.main.head;
			view.dispatch({ changes: { from: pos, insert: marker }, selection: { anchor: pos + marker.length } });
			uploading++;
			try {
				const u = await upload(f);
				const text = `${image ? '!' : ''}[${u.name.replace(/[[\]]/g, '')}](${u.url})`;
				const doc = view.state.doc.toString();
				const i = doc.indexOf(marker);
				if (i >= 0) view.dispatch({ changes: { from: i, to: i + marker.length, insert: text } });
			} catch (e) {
				const doc = view.state.doc.toString();
				const i = doc.indexOf(marker);
				if (i >= 0) view.dispatch({ changes: { from: i, to: i + marker.length, insert: '' } });
				alert(e instanceof Error ? e.message : 'The upload did not work');
			} finally {
				uploading--;
			}
		}
		focus();
	}
	function pick(kind: 'image' | 'file') {
		fileKind = kind;
		fileInput?.click();
	}

	// ---- links to pages: [[ completion and the picker ----
	function pageCompletions(ctx: CompletionContext) {
		const m = ctx.matchBefore(/\[\[[^\]\n]*/);
		if (!m || !links.length) return null;
		return {
			from: m.from,
			options: links.map((l) => ({
				label: `[[${l.title}`,
				displayLabel: l.title,
				detail: 'page',
				// Also takes the "]]" typed (or closed automatically) after the name.
				apply: (v: EditorView, _c: unknown, from: number, to: number) => {
					const end = v.state.sliceDoc(to, to + 2) === ']]' ? to + 2 : v.state.sliceDoc(to, to + 1) === ']' ? to + 1 : to;
					const text = `[${l.title}](${l.href})`;
					v.dispatch({ changes: { from, to: end, insert: text }, selection: { anchor: from + text.length } });
				}
			})),
			validFor: /^\[\[[^\]\n]*$/
		};
	}
	const shownLinks = $derived(links.filter((l) => l.title.toLowerCase().includes(linkQuery.trim().toLowerCase())).slice(0, 30));
	function linkTo(l: LinkTarget) {
		linkOpen = false;
		const sel = view ? view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to) : '';
		insertAt(`[${sel || l.title}](${l.href})`);
	}

	// ---- the editor ----
	const style = HighlightStyle.define([
		{ tag: t.heading1, fontSize: '1.35em', fontWeight: '700' },
		{ tag: t.heading2, fontSize: '1.2em', fontWeight: '700' },
		{ tag: t.heading3, fontSize: '1.08em', fontWeight: '700' },
		{ tag: [t.heading4, t.heading5, t.heading6], fontWeight: '700' },
		{ tag: t.strong, fontWeight: '700' },
		{ tag: t.emphasis, fontStyle: 'italic' },
		{ tag: t.strikethrough, textDecoration: 'line-through' },
		{ tag: t.link, color: '#4f46e5', textDecoration: 'underline' },
		{ tag: t.url, color: '#64748b' },
		{ tag: [t.processingInstruction, t.monospace], fontFamily: 'ui-monospace, monospace', color: '#be185d' },
		{ tag: t.quote, color: '#475569', fontStyle: 'italic' },
		{ tag: [t.list, t.contentSeparator], color: '#4f46e5' },
		{ tag: t.meta, color: '#94a3b8' },
		{ tag: [t.keyword, t.operator], color: '#7c3aed' },
		{ tag: [t.string, t.special(t.string)], color: '#047857' },
		{ tag: [t.number, t.bool, t.atom], color: '#b45309' },
		{ tag: t.comment, color: '#94a3b8', fontStyle: 'italic' },
		{ tag: [t.typeName, t.className], color: '#0369a1' },
		{ tag: t.function(t.variableName), color: '#1d4ed8' }
	]);
	const keys = keymap.of([
		{ key: 'Mod-b', run: () => (wrap('**'), true) },
		{ key: 'Mod-i', run: () => (wrap('_'), true) },
		{ key: 'Mod-Shift-x', run: () => (wrap('~~'), true) },
		{ key: 'Mod-e', run: () => (wrap('`'), true) },
		{ key: 'Mod-k', run: () => (link(), true) },
		{ key: 'Mod-Alt-1', run: () => (heading(1), true) },
		{ key: 'Mod-Alt-2', run: () => (heading(2), true) },
		{ key: 'Mod-Alt-3', run: () => (heading(3), true) },
		{ key: 'Mod-Shift-8', run: () => (prefix('- ', LINE_MARK), true) },
		{ key: 'Mod-Shift-7', run: () => (prefix((i) => `${i + 1}. `, LINE_MARK), true) },
		{ key: 'Mod-Shift-9', run: () => (prefix('- [ ] ', LINE_MARK), true) },
		{ key: 'Mod-s', preventDefault: true, run: () => (onsave?.(), true) }
	]);
	function extensions(): Extension[] {
		return [
			history(),
			drawSelection(),
			dropCursor(),
			indentOnInput(),
			bracketMatching(),
			closeBrackets(),
			highlightActiveLine(),
			highlightSelectionMatches(),
			search({ top: true }),
			autocompletion({ override: [pageCompletions], activateOnTyping: true }),
			markdown({ base: markdownLanguage, codeLanguages: languages }),
			syntaxHighlighting(style),
			EditorView.lineWrapping,
			cmPlaceholder(placeholder),
			EditorView.contentAttributes.of({ 'aria-label': label, spellcheck: 'true', autocorrect: 'on', autocapitalize: 'sentences' }),
			keys,
			keymap.of([...closeBracketsKeymap, ...markdownKeymap, ...defaultKeymap, ...historyKeymap, ...searchKeymap, ...completionKeymap, indentWithTab]),
			EditorView.updateListener.of((u) => {
				if (u.docChanged) {
					const v = u.state.doc.toString();
					value = v;
					refresh(v);
				}
			}),
			EditorView.domEventHandlers({
				paste: (e) => {
					const files = [...(e.clipboardData?.files ?? [])];
					if (files.length && upload) {
						e.preventDefault();
						put(files);
						return true;
					}
					return false;
				},
				drop: (e, v) => {
					const files = [...(e.dataTransfer?.files ?? [])];
					if (!files.length || !upload) return false;
					e.preventDefault();
					const pos = v.posAtCoords({ x: e.clientX, y: e.clientY }) ?? v.state.selection.main.head;
					put(files, pos);
					return true;
				},
				scroll: () => syncScroll()
			}),
			EditorView.theme({
				'&': { height: '100%', fontSize: '14px', backgroundColor: 'white' },
				'.cm-scroller': { fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', lineHeight: '1.6', padding: '8px 0' },
				'.cm-content': { padding: '0 14px', caretColor: '#4f46e5' },
				'&.cm-focused': { outline: 'none' },
				'.cm-activeLine': { backgroundColor: '#f8fafc' },
				'.cm-selectionBackground, &.cm-focused .cm-selectionBackground': { backgroundColor: '#e0e7ff !important' },
				'.cm-panels': { backgroundColor: '#f8fafc', borderColor: '#e2e8f0' },
				'.cm-tooltip-autocomplete': { borderRadius: '8px', border: '1px solid #e2e8f0' }
			})
		];
	}

	onMount(() => {
		view = new EditorView({ state: EditorState.create({ doc: untrack(() => value), extensions: extensions() }), parent: host! });
		refresh(untrack(() => value));
		const narrow = window.matchMedia('(max-width: 1023px)');
		if (narrow.matches) mode = 'write';
	});
	onDestroy(() => {
		clearTimeout(timer);
		view?.destroy();
	});
	// A value set from outside (another version restored): the editor follows.
	$effect(() => {
		const v = value;
		if (view && v !== view.state.doc.toString()) {
			view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: v } });
		}
	});

	// In split view the preview follows the editor's scroll position.
	function syncScroll() {
		if (mode !== 'split' || !view || !previewEl) return;
		const s = view.scrollDOM;
		const ratio = s.scrollTop / Math.max(1, s.scrollHeight - s.clientHeight);
		previewEl.scrollTop = ratio * (previewEl.scrollHeight - previewEl.clientHeight);
	}

	type Tool = { icon?: IconName; text?: string; title: string; run: () => void; key?: string };
	const tools: (Tool | '|')[] = [
		{ text: 'H1', title: 'Heading 1', key: 'Ctrl+Alt+1', run: () => heading(1) },
		{ text: 'H2', title: 'Heading 2', key: 'Ctrl+Alt+2', run: () => heading(2) },
		{ text: 'H3', title: 'Heading 3', key: 'Ctrl+Alt+3', run: () => heading(3) },
		'|',
		{ text: 'B', title: 'Bold', key: 'Ctrl+B', run: () => wrap('**') },
		{ text: 'I', title: 'Italic', key: 'Ctrl+I', run: () => wrap('_') },
		{ text: 'S', title: 'Strikethrough', key: 'Ctrl+Shift+X', run: () => wrap('~~') },
		{ text: '</>', title: 'Inline code', key: 'Ctrl+E', run: () => wrap('`', '`', 'code') },
		'|',
		{ text: '•', title: 'Bulleted list', key: 'Ctrl+Shift+8', run: () => prefix('- ', LINE_MARK) },
		{ text: '1.', title: 'Numbered list', key: 'Ctrl+Shift+7', run: () => prefix((i) => `${i + 1}. `, LINE_MARK) },
		{ text: '☐', title: 'Checklist', key: 'Ctrl+Shift+9', run: () => prefix('- [ ] ', LINE_MARK) },
		{ text: '❝', title: 'Quote', run: () => prefix('> ', LINE_MARK) },
		'|',
		{ icon: 'link', title: 'Link', key: 'Ctrl+K', run: link },
		{ text: '{ }', title: 'Code block', run: codeBlock },
		{ text: '▦', title: 'Table', run: table },
		{ text: '—', title: 'Divider', run: () => block('---') },
		{ text: '¹', title: 'Footnote', run: footnote }
	];
</script>

<div class="flex flex-col overflow-hidden rounded-2xl border border-slate-200 bg-white {full ? 'fixed inset-0 z-50 rounded-none border-0' : size}" data-markdown-editor>
	<div class="flex flex-wrap items-center gap-0.5 border-b border-slate-100 bg-slate-50 px-2 py-1.5" role="toolbar" aria-label="Formatting">
		{#each tools as tool, i (i)}
			{#if tool === '|'}
				<span class="mx-1 h-5 w-px bg-slate-200" aria-hidden="true"></span>
			{:else}
				<button type="button" class="flex h-8 min-w-8 items-center justify-center rounded-md px-1.5 text-sm text-slate-700 hover:bg-white hover:shadow-sm {tool.text === 'B' ? 'font-bold' : tool.text === 'I' ? 'italic' : tool.text === 'S' ? 'line-through' : ''}" title="{tool.title}{tool.key ? ` (${tool.key})` : ''}" aria-label={tool.title} onclick={tool.run}>
					{#if tool.icon}<Icon name={tool.icon} class="size-4" />{:else}{tool.text}{/if}
				</button>
			{/if}
		{/each}
		<div class="relative">
			<button type="button" class="flex h-8 items-center rounded-md px-2 text-sm text-slate-700 hover:bg-white hover:shadow-sm" aria-haspopup="menu" aria-expanded={calloutOpen} title="Callout" onclick={() => (calloutOpen = !calloutOpen)}>ⓘ ▾</button>
			{#if calloutOpen}
				<div class="absolute left-0 top-9 z-20 w-40 rounded-xl border border-slate-200 bg-white p-1 shadow-lg" role="menu">
					{#each CALLOUTS as [k, l] (k)}<button type="button" role="menuitem" class="block w-full rounded-lg px-3 py-1.5 text-left text-sm hover:bg-slate-50" onclick={() => callout(k)}>{l}</button>{/each}
				</div>
			{/if}
		</div>
		{#if upload}
			<span class="mx-1 h-5 w-px bg-slate-200" aria-hidden="true"></span>
			<button type="button" class="flex h-8 items-center gap-1 rounded-md px-2 text-sm text-slate-700 hover:bg-white hover:shadow-sm" title="Picture (or paste / drop one)" onclick={() => pick('image')}><Icon name="image" class="size-4" /> Picture</button>
			<button type="button" class="flex h-8 items-center gap-1 rounded-md px-2 text-sm text-slate-700 hover:bg-white hover:shadow-sm" title="Attach a file" onclick={() => pick('file')}><Icon name="paperclip" class="size-4" /> File</button>
		{/if}
		{#if links.length}
			<button type="button" class="flex h-8 items-center gap-1 rounded-md px-2 text-sm text-slate-700 hover:bg-white hover:shadow-sm" title="Link to a page (or type [[)" onclick={() => ((linkQuery = ''), (linkOpen = true))}>⇢ Page</button>
		{/if}
		<span class="flex-1"></span>
		<button type="button" class="flex h-8 w-8 items-center justify-center rounded-md text-slate-600 hover:bg-white hover:shadow-sm" title="Undo (Ctrl+Z)" aria-label="Undo" onclick={() => view && undo(view)}>↶</button>
		<button type="button" class="flex h-8 w-8 items-center justify-center rounded-md text-slate-600 hover:bg-white hover:shadow-sm" title="Redo (Ctrl+Shift+Z)" aria-label="Redo" onclick={() => view && redo(view)}>↷</button>
		<button type="button" class="flex h-8 w-8 items-center justify-center rounded-md text-slate-600 hover:bg-white hover:shadow-sm" title="Find and replace (Ctrl+F)" aria-label="Find and replace" onclick={() => view && openSearchPanel(view)}><Icon name="search" class="size-4" /></button>
		<div class="ml-1 flex rounded-lg bg-slate-200/70 p-0.5 text-xs" role="radiogroup" aria-label="View">
			{#each [['write', 'Write'], ['split', 'Split'], ['preview', 'Preview']] as [m, l] (m)}
				<button type="button" role="radio" aria-checked={mode === m} class="rounded-md px-2.5 py-1 {mode === m ? 'bg-white font-medium text-slate-900 shadow-sm' : 'text-slate-600'} {m === 'split' ? 'hidden lg:block' : ''}" onclick={() => (mode = m as Mode)}>{l}</button>
			{/each}
		</div>
		<button type="button" class="ml-1 flex h-8 w-8 items-center justify-center rounded-md text-slate-600 hover:bg-white hover:shadow-sm" title={full ? 'Leave full screen' : 'Full screen'} aria-label={full ? 'Leave full screen' : 'Full screen'} onclick={() => (full = !full)}>{full ? '⤡' : '⤢'}</button>
		<button type="button" class="flex h-8 w-8 items-center justify-center rounded-md text-slate-600 hover:bg-white hover:shadow-sm" title="Markdown help" aria-label="Markdown help" onclick={() => (helpOpen = !helpOpen)}>?</button>
	</div>

	<div class="grid min-h-0 flex-1 {mode === 'split' ? 'grid-cols-2 divide-x divide-slate-100' : 'grid-cols-1'}">
		<div class="min-h-0 {mode === 'preview' ? 'hidden' : ''}" bind:this={host} data-editor-source></div>
		{#if mode !== 'write'}
			<div class="min-h-0 overflow-auto px-6 py-4 {rendering ? 'opacity-80' : ''}" bind:this={previewEl} aria-label="Preview" data-editor-preview>
				{#if value.trim()}<MarkdownView {html} />{:else}<p class="text-sm text-slate-400">Nothing to preview yet.</p>{/if}
			</div>
		{/if}
	</div>

	<div class="flex items-center gap-3 border-t border-slate-100 bg-slate-50 px-3 py-1 text-xs text-slate-500">
		<span data-editor-stats>{stats.words} words · {stats.chars} characters · {stats.minutes} min read</span>
		{#if uploading}<span class="text-brand-700">Uploading {uploading}…</span>{/if}
		<span class="flex-1"></span>
		<span class="hidden sm:inline">Type <code>[[</code> to link a page · paste or drop pictures</span>
	</div>
</div>

<input bind:this={fileInput} type="file" class="hidden" multiple accept={fileKind === 'image' ? 'image/*' : undefined} onchange={(e) => { const f = [...(e.currentTarget.files ?? [])]; e.currentTarget.value = ''; put(f); }} />

{#if linkOpen}
	<div class="fixed inset-0 z-[60] flex items-start justify-center bg-slate-900/30 p-4 pt-24" role="presentation" onclick={(e) => e.target === e.currentTarget && (linkOpen = false)} onkeydown={(e) => e.key === 'Escape' && (linkOpen = false)}>
		<div class="w-full max-w-md rounded-2xl bg-white p-3 shadow-g2" role="dialog" aria-label="Link to a page">
			<!-- svelte-ignore a11y_autofocus -->
			<input class="input" placeholder="Find a page…" bind:value={linkQuery} autofocus aria-label="Find a page" onkeydown={(e) => { if (e.key === 'Enter' && shownLinks[0]) linkTo(shownLinks[0]); if (e.key === 'Escape') linkOpen = false; }} />
			<ul class="mt-2 max-h-72 overflow-auto">
				{#each shownLinks as l (l.id)}<li><button type="button" class="w-full truncate rounded-lg px-3 py-1.5 text-left text-sm hover:bg-slate-50" onclick={() => linkTo(l)}>{l.title}</button></li>{:else}<li class="px-3 py-2 text-sm text-slate-400">No page by that name.</li>{/each}
			</ul>
		</div>
	</div>
{/if}

{#if helpOpen}
	<div class="fixed inset-0 z-[60] flex items-start justify-center bg-slate-900/30 p-4 pt-16" role="presentation" onclick={(e) => e.target === e.currentTarget && (helpOpen = false)} onkeydown={(e) => e.key === 'Escape' && (helpOpen = false)}>
		<div class="max-h-[80vh] w-full max-w-2xl overflow-auto rounded-2xl bg-white p-5 shadow-g2 text-sm" role="dialog" aria-label="Markdown help">
			<div class="mb-3 flex items-center"><h2 class="flex-1 text-lg font-semibold">Markdown</h2><button class="btn btn-ghost px-2" onclick={() => (helpOpen = false)} aria-label="Close">✕</button></div>
			<table class="w-full text-left">
				<tbody class="divide-y divide-slate-100 align-top">
					{#each [
						['# Heading, ## Smaller, ### Smaller still', 'Headings (each gets a link: hover it to copy)'],
						['**bold**  _italic_  ~~struck~~  `code`', 'Text styles'],
						['- item   1. item   - [ ] to do   - [x] done', 'Lists and checklists (Enter continues them)'],
						['> quoted text', 'A quote'],
						['> [!NOTE]  > [!TIP]  > [!WARNING] …', 'Callouts (first line of a quote)'],
						['[text](https://…)   [text](/kb/page/…)   [[', 'Links; [[ finds a page'],
						['![description](picture address)', 'A picture (or paste / drop one)'],
						['| a | b |\\n| --- | --- |\\n| 1 | 2 |', 'A table (:--- :---: ---: align columns)'],
						['```sql … ```', 'Code with highlighting (name the language)'],
						['Text[^1] … [^1]: The note.', 'A footnote'],
						['---', 'A dividing line']
					] as [md, what] (md)}
						<tr><td class="py-1.5 pr-4 font-mono text-xs whitespace-pre-wrap text-slate-700">{md}</td><td class="py-1.5 text-slate-600">{what}</td></tr>
					{/each}
				</tbody>
			</table>
			<p class="mt-3 text-xs text-slate-500">Shortcuts: Ctrl+B bold, Ctrl+I italic, Ctrl+K link, Ctrl+E code, Ctrl+Alt+1–3 headings, Ctrl+Shift+7/8/9 lists, Ctrl+F find and replace, Ctrl+S save, Tab to indent.</p>
		</div>
	</div>
{/if}
