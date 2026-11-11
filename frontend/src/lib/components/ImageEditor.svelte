<script lang="ts">
	// A small picture editor: turn, flip, crop to a shape (from the middle) and make smaller.
	// The result is a new picture; the original stays as it was.
	import Modal from '#lib/components/Modal.svelte';

	let {
		open = $bindable(false),
		source,
		ondone
	}: { open: boolean; source: string | null; ondone: (f: File) => void } = $props();
	let turn = $state(0);
	let flip = $state(false);
	let shape = $state('original');
	let width = $state(0);
	let img = $state<HTMLImageElement | null>(null);
	let canvas: HTMLCanvasElement | undefined = $state();
	const SHAPES: [string, string, number][] = [
		['original', 'As it is', 0],
		['square', 'Square', 1],
		['4:3', '4:3', 4 / 3],
		['16:9', '16:9 (banner)', 16 / 9],
		['3:4', '3:4 (portrait)', 3 / 4]
	];

	$effect(() => {
		if (!open || !source) return;
		turn = 0;
		flip = false;
		shape = 'original';
		width = 0;
		const i = new Image();
		i.onload = () => (img = i);
		i.src = source;
	});

	$effect(() => {
		if (!img || !canvas) return;
		const sideways = turn % 180 !== 0;
		let w = sideways ? img.naturalHeight : img.naturalWidth;
		let h = sideways ? img.naturalWidth : img.naturalHeight;
		// The crop, in turned coordinates, from the middle.
		const ratio = SHAPES.find(([k]) => k === shape)![2];
		let cw = w,
			ch = h;
		if (ratio) {
			if (w / h > ratio) cw = Math.round(h * ratio);
			else ch = Math.round(w / ratio);
		}
		const scale = width && width < cw ? width / cw : 1;
		canvas.width = Math.round(cw * scale);
		canvas.height = Math.round(ch * scale);
		const ctx = canvas.getContext('2d')!;
		ctx.save();
		ctx.scale(scale, scale);
		ctx.translate(cw / 2, ch / 2);
		ctx.rotate((turn * Math.PI) / 180);
		if (flip) ctx.scale(-1, 1);
		ctx.drawImage(img, -img.naturalWidth / 2, -img.naturalHeight / 2);
		ctx.restore();
		void w;
		void h;
	});

	function done() {
		canvas!.toBlob(
			(b) => {
				if (b) ondone(new File([b], 'picture.jpg', { type: 'image/jpeg' }));
				open = false;
			},
			'image/jpeg',
			0.9
		);
	}
</script>

<Modal bind:open title="Edit the picture" size="lg">
	<div class="space-y-3 text-sm">
		<div class="flex max-h-[50vh] justify-center overflow-hidden rounded-lg bg-slate-100 p-2">
			<canvas bind:this={canvas} class="max-h-[48vh] max-w-full object-contain" data-image-canvas
			></canvas>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<button
				class="btn btn-secondary"
				onclick={() => (turn = (turn + 270) % 360)}
				aria-label="Turn left">⟲</button
			>
			<button
				class="btn btn-secondary"
				onclick={() => (turn = (turn + 90) % 360)}
				aria-label="Turn right">⟳</button
			>
			<button class="btn btn-secondary" aria-pressed={flip} onclick={() => (flip = !flip)}
				>Mirror</button
			>
			<select class="input w-auto" aria-label="Shape" bind:value={shape}
				>{#each SHAPES as [k, l] (k)}<option value={k}>{l}</option>{/each}</select
			>
			<select class="input w-auto" aria-label="Size" bind:value={width}
				><option value={0}>Full size</option><option value={1920}>1920 wide</option><option
					value={1200}>1200 wide</option
				><option value={800}>800 wide</option><option value={400}>400 wide</option></select
			>
		</div>
	</div>
	{#snippet footer()}
		<button class="btn btn-secondary" onclick={() => (open = false)}>Cancel</button>
		<button class="btn btn-primary" onclick={done} disabled={!img}>Use it</button>
	{/snippet}
</Modal>
