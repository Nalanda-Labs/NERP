<script lang="ts">
	// Drawing a signature with mouse, finger or pen; `drawn` tells whether anything is drawn
	// and `png()` gives it as a data: URL.
	let { drawn = $bindable(false) }: { drawn?: boolean } = $props();
	let canvas: HTMLCanvasElement | undefined = $state();
	let drawing = false;
	function ctx() {
		const c = canvas!.getContext('2d')!;
		c.lineWidth = 2.5;
		c.lineCap = 'round';
		c.lineJoin = 'round';
		c.strokeStyle = '#0f172a';
		return c;
	}
	function point(e: PointerEvent) {
		const r = canvas!.getBoundingClientRect();
		return [((e.clientX - r.left) * canvas!.width) / r.width, ((e.clientY - r.top) * canvas!.height) / r.height];
	}
	function down(e: PointerEvent) {
		drawing = true;
		canvas!.setPointerCapture(e.pointerId);
		const [x, y] = point(e);
		const c = ctx();
		c.beginPath();
		c.moveTo(x, y);
		c.lineTo(x + 0.1, y + 0.1);
		c.stroke();
		drawn = true;
	}
	function move(e: PointerEvent) {
		if (!drawing) return;
		const [x, y] = point(e);
		const c = ctx();
		c.lineTo(x, y);
		c.stroke();
	}
	function up() {
		drawing = false;
	}
	export function clear() {
		ctx().clearRect(0, 0, canvas!.width, canvas!.height);
		drawn = false;
	}
	export function png() {
		return canvas!.toDataURL('image/png');
	}
</script>

<div>
	<div class="mb-1 flex items-center justify-between"><span class="label mb-0">Your signature</span><button type="button" class="text-xs text-slate-500 hover:text-slate-800" onclick={clear}>Clear</button></div>
	<canvas bind:this={canvas} width="900" height="240" class="h-32 w-full touch-none rounded-xl border border-dashed border-slate-300 bg-slate-50" aria-label="Draw your signature" onpointerdown={down} onpointermove={move} onpointerup={up} onpointerleave={up}></canvas>
	<p class="mt-1 text-xs text-slate-500">Draw with your mouse, finger or pen.</p>
</div>
