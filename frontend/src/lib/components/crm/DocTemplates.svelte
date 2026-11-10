<script lang="ts">
	// CRM settings → Documents: templates in Typst, LuaLaTeX or Word/Writer format, with a
	// field reference and a live PDF preview.
	import { onDestroy, onMount } from 'svelte';
	import { api, downloadFile, requestBlob } from '#lib/api.ts';
	import { crm } from '#lib/crm.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import Icon from '../Icon.svelte';
	import CrmPicker from './CrmPicker.svelte';

	type Engine = 'typst' | 'latex' | 'office';
	type Template = {
		id: string;
		name: string;
		engine: Engine;
		entity_types: number[];
		doc_title: string;
		locale: string;
		source: string | null;
		file_id: string | null;
		number_prefix: string;
		builtin_key: string | null;
		is_active: boolean;
		available: boolean;
	};
	type Engines = { typst: string | null; latex: string | null; office: string | null };
	type Draft = Omit<Template, 'id' | 'builtin_key' | 'available' | 'entity_types'> & { id: string | null; types: string[] };

	const ENGINE_NAME: Record<Engine, string> = { typst: 'Typst', latex: 'LuaLaTeX', office: 'Word / Writer' };
	const STARTER: Record<Exclude<Engine, 'office'>, string> = {
		typst: `#let d = json("data.json")
#set page(paper: "a4", margin: 2cm)
#set text(font: ("Noto Sans", "Noto Sans Devanagari", "Noto Sans Arabic", "Noto Sans CJK SC"), size: 10pt)

= #d.doc.title #d.doc.number
#d.doc.date

*#d.client.name* \\
#d.client.address

#table(
  columns: (1fr, auto, auto),
  [*Item*], [*Qty*], [*Amount*],
  ..d.products.map(p => ([#p.name], [#p.quantity #p.unit], [#p.total.text])).flatten(),
)

#align(right)[*Total #d.totals.total.text*]
`,
		latex: `\\documentclass[a4paper,10pt]{article}
\\usepackage{fontspec}
\\setmainfont{Noto Sans}
\\usepackage[margin=2cm]{geometry}
\\usepackage{nerp}
\\begin{document}
\\section*{\\crm{doc.title} \\crm{doc.number}}
\\crm{doc.date}

\\textbf{\\crm{client.name}}\\\\
\\crm{client.address}

\\begin{tabular}{lrr}
Item & Qty & Amount \\\\ \\hline
\\crmrows{products}{\\crmrow{name} & \\crmrow{quantity} \\crmrow{unit} & \\crmrow{total} \\\\}
\\end{tabular}

\\crmif{totals.has_tax}{Tax: \\crm{totals.tax}}
\\crmifnot{has_products}{No products}

\\textbf{Total \\crm{totals.total}}
\\end{document}
`
	};

	let templates = $state<Template[]>([]);
	let engines = $state<Engines>({ typst: 'built in', latex: null, office: null });
	let draft = $state<Draft | null>(null);
	let saved = $state('');
	let sample = $state<Record<string, unknown> | null>(null);
	let previewUrl = $state<string | null>(null);
	let previewError = $state<string | null>(null);
	let previewing = $state(false);
	let saving = $state(false);
	let editor: HTMLTextAreaElement | undefined = $state();

	const types = $derived(crm.config?.types ?? []);
	// Preview with a real record's data instead of the sample.
	let previewItem = $state<{ id: string; title: string; type: string; number: number } | null>(null);
	const typeName = (id: number) => types.find((t) => t.id === id)?.name_plural ?? '';
	const dirty = $derived(draft !== null && JSON.stringify(draft) !== saved);

	async function load() {
		const r = await api.get<{ items: Template[]; engines: Engines }>('/crm/doc-templates');
		templates = r.items;
		engines = r.engines;
	}
	onMount(async () => {
		await load();
		sample = await api.get('/crm/doc-templates/sample');
	});
	onDestroy(() => previewUrl && URL.revokeObjectURL(previewUrl));

	function open(t: Template) {
		if (dirty && !confirm('Discard unsaved changes?')) return;
		draft = {
			id: t.id,
			name: t.name,
			engine: t.engine,
			types: t.entity_types.map((id) => types.find((x) => x.id === id)?.code ?? '').filter(Boolean),
			doc_title: t.doc_title,
			locale: t.locale,
			source: t.source,
			file_id: t.file_id,
			number_prefix: t.number_prefix,
			is_active: t.is_active
		};
		saved = JSON.stringify(draft);
		clearPreview();
		if (t.engine !== 'office') schedulePreview(0);
	}

	function startNew(engine: Engine) {
		if (dirty && !confirm('Discard unsaved changes?')) return;
		draft = {
			id: null,
			name: '',
			engine,
			types: ['deal'],
			doc_title: 'Invoice',
			locale: '',
			source: engine === 'office' ? null : STARTER[engine],
			file_id: null,
			number_prefix: 'INV-{YYYY}-',
			is_active: true
		};
		saved = '';
		clearPreview();
		if (engine !== 'office') schedulePreview(0);
	}

	function clearPreview() {
		if (previewUrl) URL.revokeObjectURL(previewUrl);
		previewUrl = null;
		previewError = null;
	}

	// Live preview: re-render a moment after the author stops typing. Typst takes tens of
	// milliseconds; LaTeX a second or two.
	let timer: ReturnType<typeof setTimeout>;
	let seq = 0;
	function schedulePreview(delay = draft?.engine === 'typst' ? 500 : 1500) {
		clearTimeout(timer);
		timer = setTimeout(preview, delay);
	}
	async function preview() {
		if (!draft) return;
		const mine = ++seq;
		previewing = true;
		try {
			const blob = await requestBlob('POST', '/crm/doc-templates/preview', {
				engine: draft.engine,
				source: draft.source,
				file_id: draft.file_id,
				locale: draft.locale,
				doc_title: draft.doc_title,
				item_id: previewItem?.id
			});
			if (mine !== seq) return;
			if (previewUrl) URL.revokeObjectURL(previewUrl);
			previewUrl = URL.createObjectURL(blob);
			previewError = null;
		} catch (e) {
			if (mine === seq) previewError = (e as Error).message;
		} finally {
			if (mine === seq) previewing = false;
		}
	}

	async function save() {
		if (!draft) return;
		saving = true;
		try {
			const body = { name: draft.name, engine: draft.engine, entity_types: draft.types, doc_title: draft.doc_title, locale: draft.locale, source: draft.source ?? undefined, file_id: draft.file_id ?? undefined, number_prefix: draft.number_prefix, is_active: draft.is_active };
			const t = draft.id ? await api.patch<Template>(`/crm/doc-templates/${draft.id}`, body) : await api.post<Template>('/crm/doc-templates', body);
			await load();
			saved = '';
			open(t);
			toasts.success('Template saved');
		} catch (e) {
			toasts.error(e);
		} finally {
			saving = false;
		}
	}

	async function remove() {
		if (!draft?.id || !confirm(`Delete the template "${draft.name}"? Documents already created keep their PDFs.`)) return;
		try {
			await api.del(`/crm/doc-templates/${draft.id}`);
			draft = null;
			saved = '';
			clearPreview();
			await load();
		} catch (e) {
			toasts.error(e);
		}
	}

	/** A file name from the template name, without characters file systems reject. */
	function fileName(ext: string) {
		const base = (draft?.name || 'template').replace(/[\\/:*?"<>|\p{Cc}]+/gu, '-').replace(/\s+/g, ' ').trim() || 'template';
		return `${base}.${ext}`;
	}
	function downloadSource() {
		if (!draft) return;
		const href = URL.createObjectURL(new Blob([draft.source ?? ''], { type: 'text/plain;charset=utf-8' }));
		const a = document.createElement('a');
		a.href = href;
		a.download = fileName(draft.engine === 'typst' ? 'typ' : 'tex');
		document.body.appendChild(a);
		a.click();
		a.remove();
		setTimeout(() => URL.revokeObjectURL(href), 60_000);
	}
	async function openSource(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const f = input.files?.[0];
		input.value = '';
		if (!f || !draft) return;
		if (f.size > 1_000_000) return toasts.error(new Error('The file is larger than 1 MB'));
		const text = await f.text();
		if (draft.source && draft.source !== text && !confirm(`Replace the current source with ${f.name}? Unsaved changes are lost.`)) return;
		draft.source = text;
		schedulePreview(0);
	}
	async function upload(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		if (!file || !draft) return;
		const fd = new FormData();
		fd.append('file', file);
		try {
			const f = await api.post<{ id: string }>('/files', fd);
			draft.file_id = f.id;
			toasts.success(`${file.name} uploaded — save the template to use it`);
			schedulePreview(0);
		} catch (err) {
			toasts.error(err);
		} finally {
			input.value = '';
		}
	}

	// ----- field reference -----
	type Field = { path: string; kind: 'text' | 'money' | 'flag' | 'list'; children?: Field[] };
	function fields(obj: Record<string, unknown>, prefix = ''): Field[] {
		return Object.entries(obj).flatMap(([k, v]): Field[] => {
			const path = prefix ? `${prefix}.${k}` : k;
			if (Array.isArray(v)) {
				const first = v[0];
				return [{ path, kind: 'list', children: first && typeof first === 'object' ? fields(first as Record<string, unknown>) : [] }];
			}
			if (v && typeof v === 'object') {
				if ('text' in v && 'value' in v) return [{ path, kind: 'money' }];
				return fields(v as Record<string, unknown>, path);
			}
			return [{ path, kind: typeof v === 'boolean' ? 'flag' : 'text' }];
		});
	}
	const reference = $derived(sample ? fields(sample) : []);

	function snippet(f: Field, list?: string): string {
		const e = draft?.engine ?? 'typst';
		if (list) {
			if (e === 'typst') return f.kind === 'money' ? `#p.${f.path}.text` : `#p.${f.path}`;
			if (e === 'latex') return `\\crmrow{${f.path}}`;
			return `{{${f.path}}}`;
		}
		if (f.kind === 'list') {
			if (e === 'typst') return `#for p in d.${f.path} [\n  #p.name\n]`;
			if (e === 'latex') return `\\crmrows{${f.path}}{\\crmrow{name}\\\\}`;
			return `{{#${f.path}}}{{name}}{{/${f.path}}}`;
		}
		if (f.kind === 'flag') {
			if (e === 'typst') return `#if d.${f.path} [ … ]`;
			if (e === 'latex') return `\\crmif{${f.path}}{ … }`;
			return `{{#${f.path}}} … {{/${f.path}}}`;
		}
		if (e === 'typst') return f.kind === 'money' ? `#d.${f.path}.text` : `#d.${f.path}`;
		if (e === 'latex') return `\\crm{${f.path}}`;
		return `{{${f.path}}}`;
	}

	function insert(text: string) {
		if (!draft) return;
		if (draft.engine === 'office' || !editor) {
			navigator.clipboard?.writeText(text);
			toasts.success(`Copied ${text}`);
			return;
		}
		const { selectionStart: a, selectionEnd: b } = editor;
		const src = draft.source ?? '';
		draft.source = src.slice(0, a) + text + src.slice(b);
		requestAnimationFrame(() => {
			editor!.focus();
			editor!.selectionStart = editor!.selectionEnd = a + text.length;
		});
		schedulePreview();
	}

	function tabKey(e: KeyboardEvent) {
		if (e.key !== 'Tab' || !draft) return;
		e.preventDefault();
		insert('  ');
	}
</script>

<div class="grid gap-5 lg:grid-cols-[16rem_minmax(0,1fr)]">
	<!-- Template list -->
	<aside class="space-y-3">
		<section class="card">
			<ul class="divide-y divide-slate-50">
				{#each templates as t (t.id)}
					<li>
						<button class="flex w-full items-start gap-2 px-4 py-2.5 text-left text-sm hover:bg-slate-50 {draft?.id === t.id ? 'bg-brand-50' : ''}" onclick={() => open(t)}>
							<Icon name="file" class="mt-0.5 size-4 shrink-0 {t.is_active ? 'text-brand-600' : 'text-slate-300'}" />
							<span class="min-w-0 flex-1">
								<span class="block truncate font-medium {t.is_active ? 'text-slate-800' : 'text-slate-400'}">{t.name}</span>
								<span class="block truncate text-xs text-slate-500">{ENGINE_NAME[t.engine]} · {t.entity_types.map(typeName).join(', ')}</span>
								{#if !t.available}<span class="block text-xs text-amber-700">Engine not installed</span>{/if}
							</span>
						</button>
					</li>
				{:else}
					<li class="px-4 py-3 text-sm text-slate-400">No templates yet.</li>
				{/each}
			</ul>
		</section>
		<section class="card space-y-1 p-3">
			<p class="px-1 pb-1 text-xs font-medium text-slate-500">New template</p>
			{#each ['typst', 'latex', 'office'] as const as e (e)}
				<button class="btn btn-ghost w-full justify-start text-sm" disabled={!engines[e]} title={engines[e] ?? `${ENGINE_NAME[e]} is not installed on the server`} onclick={() => startNew(e)}>
					<Icon name="plus" class="size-4" /> {ENGINE_NAME[e]}
				</button>
			{/each}
		</section>
	</aside>

	<!-- Editor -->
	{#if draft}
		<div class="min-w-0 space-y-4">
			<section class="card space-y-4 p-5">
				<div class="grid gap-4 sm:grid-cols-2">
					<div>
						<label class="label" for="dt-name">Template name</label>
						<input id="dt-name" class="input" maxlength="200" bind:value={draft.name} placeholder="e.g. Invoice (EU)" />
					</div>
					<div>
						<label class="label" for="dt-title">Printed title</label>
						<input id="dt-title" class="input" maxlength="100" bind:value={draft.doc_title} oninput={() => schedulePreview()} placeholder="Invoice, Quote, Rechnung…" />
					</div>
					<div>
						<label class="label" for="dt-prefix">Number prefix</label>
						<input id="dt-prefix" class="input font-mono" maxlength="40" bind:value={draft.number_prefix} />
						<p class="mt-1 text-xs text-slate-500">{'{YYYY}'}, {'{YY}'} and {'{MM}'} are replaced; numbering restarts for each new prefix. Next looks like <span class="font-mono">{draft.number_prefix.replace('{YYYY}', String(new Date().getFullYear())).replace('{YY}', String(new Date().getFullYear() % 100).padStart(2, '0')).replace('{MM}', String(new Date().getMonth() + 1).padStart(2, '0'))}0001</span>.</p>
					</div>
					<div>
						<label class="label" for="dt-locale">Number &amp; date format</label>
						<input id="dt-locale" class="input" list="dt-locales" maxlength="35" bind:value={draft.locale} oninput={() => schedulePreview()} placeholder="Company default" />
						<datalist id="dt-locales">
							{#each ['en-US', 'en-GB', 'en-IN', 'de-DE', 'fr-FR', 'es-ES', 'it-IT', 'pt-BR', 'nl-NL', 'pl-PL', 'ru-RU', 'tr-TR', 'ar-SA', 'hi-IN', 'zh-CN', 'ja-JP', 'ko-KR', 'de-CH'] as l (l)}<option value={l}></option>{/each}
						</datalist>
					</div>
				</div>
				<div class="flex flex-wrap items-center gap-x-5 gap-y-2 text-sm">
					<span class="text-slate-500">Offered on</span>
					{#each types as t (t.code)}
						<label class="flex items-center gap-1.5">
							<input type="checkbox" class="rounded border-slate-300 text-brand-600" checked={draft.types.includes(t.code)} onchange={(e) => (draft!.types = e.currentTarget.checked ? [...draft!.types, t.code] : draft!.types.filter((c) => c !== t.code))} />
							{t.name_plural}
						</label>
					{/each}
					<label class="ml-auto flex items-center gap-1.5"><input type="checkbox" class="rounded border-slate-300 text-brand-600" bind:checked={draft.is_active} /> Active</label>
				</div>
			</section>

			<div class="grid gap-4 xl:grid-cols-2">
				<!-- Source or file -->
				<section class="card flex min-h-[32rem] flex-col">
					<div class="flex items-center justify-between border-b border-slate-100 px-4 py-2.5">
						<h3 class="text-sm font-medium text-slate-900">{ENGINE_NAME[draft.engine]} {draft.engine === 'office' ? 'file' : 'source'}</h3>
						{#if draft.engine !== 'office'}
							<div class="flex items-center gap-1">
								<label class="btn btn-ghost cursor-pointer px-2 py-1 text-xs" title="Replace the source with a .{draft.engine === 'typst' ? 'typ' : 'tex'} file from your computer"><Icon name="upload" class="size-3.5" /> Open file<input type="file" class="sr-only" accept={draft.engine === 'typst' ? '.typ,text/plain' : '.tex,text/plain'} onchange={openSource} /></label>
								<button class="btn btn-ghost px-2 py-1 text-xs" title="Download the source as a file" onclick={downloadSource}><Icon name="download" class="size-3.5" /> Download</button>
								<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => schedulePreview(0)}><Icon name="refresh" class="size-3.5" /> Preview</button>
							</div>
						{/if}
					</div>
					{#if draft.engine === 'office'}
						<div class="space-y-4 p-4 text-sm">
							<div class="flex flex-wrap gap-2">
								<label class="btn btn-secondary cursor-pointer"><Icon name="upload" class="size-4" /> Upload .docx or .odt<input type="file" class="sr-only" accept=".docx,.odt,application/vnd.openxmlformats-officedocument.wordprocessingml.document,application/vnd.oasis.opendocument.text" onchange={upload} /></label>
								{#if draft.file_id}
									<button class="btn btn-ghost" onclick={() => downloadFile(draft!.file_id!, fileName('docx')).catch(toasts.error)}><Icon name="download" class="size-4" /> Download current</button>
									<button class="btn btn-ghost" onclick={() => schedulePreview(0)}><Icon name="eye" class="size-4" /> Preview</button>
								{/if}
							</div>
							<div class="rounded-lg bg-slate-50 p-3 text-xs leading-relaxed text-slate-600">
								<p class="mb-1 font-medium text-slate-800">How to write a Word template</p>
								<ul class="list-disc space-y-1 pl-4">
									<li>Type placeholders anywhere: <code>{'{{client.name}}'}</code>, <code>{'{{totals.total}}'}</code>. Formatting applies to the whole placeholder.</li>
									<li>Repeat a table row per product: put <code>{'{{#products}}'}</code> in its first cell and <code>{'{{/products}}'}</code> in its last; use <code>{'{{name}}'}</code>, <code>{'{{quantity}}'}</code>, <code>{'{{total}}'}</code> inside.</li>
									<li>Show something only when set: <code>{'{{#totals.has_tax}}'}</code> … <code>{'{{/totals.has_tax}}'}</code> (works for whole table rows too).</li>
									<li>…or only when empty: <code>{'{{^products}}'}</code> … <code>{'{{/products}}'}</code>, for example a single line when a record has no products.</li>
									<li>Writer .odt files are converted to .docx when you save.</li>
									<li>Click a field on the right to copy its placeholder.</li>
								</ul>
							</div>
						</div>
					{:else}
						<textarea
							bind:this={editor}
							class="min-h-[28rem] w-full flex-1 resize-y border-0 bg-transparent p-4 font-mono text-[13px] leading-5 text-slate-800 focus:ring-0"
							spellcheck="false"
							wrap="off"
							aria-label="Template source"
							bind:value={draft.source}
							oninput={() => schedulePreview()}
							onkeydown={tabKey}
						></textarea>
					{/if}
				</section>

				<!-- Preview -->
				<section class="card flex min-h-[32rem] flex-col overflow-hidden">
					<div class="flex items-center justify-between border-b border-slate-100 px-4 py-2.5">
						<h3 class="text-sm font-medium text-slate-900">Preview <span class="font-normal text-slate-500">· {previewItem ? previewItem.title : 'sample data'}</span></h3>
						<div class="flex items-center gap-2">
							{#if previewing}<span class="text-xs text-slate-500">Rendering…</span>{/if}
							{#if previewUrl && !previewError}
								<a class="btn btn-ghost px-2 py-1 text-xs" href={previewUrl} download={fileName('pdf')} title="Download this preview as a PDF"><Icon name="download" class="size-3.5" /> Download PDF</a>
							{/if}
						</div>
					</div>
					<div class="flex flex-wrap items-center gap-2 border-b border-slate-100 px-4 py-2 text-xs text-slate-600">
						<span>Fill with</span>
						{#if previewItem}
							<span class="badge bg-brand-50 text-brand-900">{previewItem.title} · {types.find((t) => t.code === previewItem!.type)?.name ?? previewItem.type} #{previewItem.number}</span>
							<button class="btn btn-ghost px-2 py-1 text-xs" onclick={() => ((previewItem = null), schedulePreview(0))}>Use sample data</button>
						{:else}
							<span class="text-slate-400">sample data, or a real record:</span>
							<div class="min-w-56 flex-1">
								<CrmPicker
									types={draft.types.length ? draft.types : types.map((t) => t.code)}
									placeholder="Find a deal, lead…"
									onpick={(h) => ((previewItem = { id: h.id, title: h.title, type: h.type, number: h.number }), schedulePreview(0))}
								/>
							</div>
						{/if}
					</div>
					{#if previewError}
						<pre class="m-4 overflow-auto rounded-lg bg-red-50 p-3 text-xs whitespace-pre-wrap text-red-800" role="alert">{previewError}</pre>
					{/if}
					{#if previewUrl}
						<iframe title="PDF preview" src={previewUrl} class="min-h-[30rem] w-full flex-1 border-0 {previewError ? 'opacity-40' : ''}"></iframe>
					{:else if !previewError}
						<p class="p-6 text-sm text-slate-400">{draft.engine === 'office' && !draft.file_id ? 'Upload a template to preview it.' : 'The preview appears here.'}</p>
					{/if}
				</section>
			</div>

			<!-- Field reference -->
			<section class="card">
				<h3 class="border-b border-slate-100 px-4 py-2.5 text-sm font-medium text-slate-900">Fields <span class="font-normal text-slate-500">· click to {draft.engine === 'office' ? 'copy' : 'insert'}</span></h3>
				<div class="grid gap-x-4 p-3 text-xs sm:grid-cols-2 xl:grid-cols-3">
					{#each reference as f (f.path)}
						{#if f.kind === 'list'}
							<div class="col-span-full mt-2">
								<button class="rounded px-1.5 py-0.5 font-mono text-brand-700 hover:bg-brand-50" onclick={() => insert(snippet(f))}>{f.path}[]</button>
								<span class="text-slate-400">— repeat for each; inside:</span>
								<div class="mt-1 flex flex-wrap gap-1 pl-4">
									{#each f.children ?? [] as c (c.path)}
										<button class="rounded px-1.5 py-0.5 font-mono text-slate-600 hover:bg-slate-100" onclick={() => insert(snippet(c, f.path))}>{c.path}</button>
									{/each}
								</div>
							</div>
						{:else}
							<button class="truncate rounded px-1.5 py-0.5 text-left font-mono text-slate-600 hover:bg-slate-100" title={snippet(f)} onclick={() => insert(snippet(f))}>{f.path}{f.kind === 'flag' ? ' ?' : ''}</button>
						{/if}
					{/each}
				</div>
				{#if draft.engine === 'latex'}
					<p class="border-t border-slate-100 px-4 py-2.5 text-xs text-slate-500">Load <code>\usepackage{'{nerp}'}</code>. Values print literally — TeX special characters in CRM data are safe. Shell escape and file access outside the job folder are disabled.</p>
				{:else if draft.engine === 'typst'}
					<p class="border-t border-slate-100 px-4 py-2.5 text-xs text-slate-500">Read the data with <code>#let d = json("data.json")</code>. The company logo, when set, is the image <code>"logo"</code>. Money values have <code>.text</code> (formatted) and <code>.value</code> (number).</p>
				{/if}
			</section>

			<div class="flex flex-wrap items-center gap-2">
				<button class="btn btn-primary" disabled={saving || !draft.name.trim() || !draft.types.length || (draft.engine === 'office' && !draft.file_id)} onclick={save}>{saving ? 'Saving…' : draft.id ? 'Save template' : 'Create template'}</button>
				{#if dirty && draft.id}<span class="text-xs text-amber-700">Unsaved changes</span>{/if}
				{#if draft.id}<button class="btn btn-ghost ml-auto text-red-600" onclick={remove}><Icon name="trash" class="size-4" /> Delete template</button>{/if}
			</div>
		</div>
	{:else}
		<div class="card p-10 text-center text-sm text-slate-500">
			<p>Pick a template to edit it, or create a new one.</p>
			<p class="mt-2 text-xs text-slate-400">
				Engines on this server: Typst (built in){engines.latex ? ' · LuaLaTeX' : ''}{engines.office ? ' · LibreOffice' : ''}
			</p>
		</div>
	{/if}
</div>
