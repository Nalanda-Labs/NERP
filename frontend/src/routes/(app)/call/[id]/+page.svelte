<script lang="ts">
	// A video call: every browser connects to every other (WebRTC; NERP passes the setup
	// messages along), or, when the call goes through the LiveKit media server, everyone
	// connects to that. Camera and microphone on/off, screen sharing, recording (LiveKit), leave.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onDestroy, onMount } from 'svelte';
	import { api, sendOnUnload } from '#lib/api.ts';
	import { blurTrack, type Blurred } from '#lib/blur.ts';
	import Icon from '#lib/components/Icon.svelte';
	import { realtime } from '#lib/realtime.svelte.ts';
	import { session } from '#lib/session.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';

	type Remote = { peerId: string; userId: string; name: string; pc: RTCPeerConnection | null; stream: MediaStream | null; state: string; pending: RTCIceCandidateInit[] };
	type Recording = { id: string; started_by: string | null; started_by_name: string | null; started_at: string } | null;
	const id = $derived(page.params.id ?? '');
	type Allow = { audio: boolean; video: boolean; screen: boolean };
	let callInfo = $state<{ chat_id: string | null; ended_at: string | null; people: { id: string; name: string }[]; media: 'p2p' | 'livekit'; can_record: boolean; recording: Recording; host_id: string | null; allow: Allow } | null>(null);
	/** What the host lets the others use. */
	let allow = $state<Allow>({ audio: true, video: true, screen: true });
	const isHost = $derived(!!callInfo && callInfo.host_id === session.user?.id);
	let hostOpen = $state(false);
	async function setControls(body: Partial<{ allow_audio: boolean; allow_video: boolean; allow_screen: boolean; mute_all: boolean }>) {
		try {
			allow = (await api.post<{ allow: Allow }>(`/calls/${id}/controls`, body)).allow;
			if (body.mute_all) toasts.success('Everyone else is muted');
		} catch (e) {
			toasts.error(e);
		}
	}
	/** The host changed what I may use, or muted everyone. */
	function onControls(d: { call_id: string; allow: Allow; mute_all: boolean }) {
		if (d.call_id !== id || isHost) return;
		allow = d.allow;
		if ((d.mute_all || !d.allow.audio) && mic) toggle('audio');
		if (!d.allow.video && cam) toggle('video');
		if (!d.allow.screen && sharing) stopShare();
		if (d.mute_all) toasts.info('The host muted everyone');
	}
	let recording = $state<Recording>(null);
	let room: import('livekit-client').Room | null = null;
	let local = $state<MediaStream | null>(null);
	let peerId = $state<string | null>(null);
	let remotes = $state<Remote[]>([]);
	let mic = $state(true);
	let cam = $state(true);
	let sharing = $state<MediaStream | null>(null);
	let status = $state<'starting' | 'in' | 'left' | 'ended' | 'failed'>('starting');
	let ice: RTCIceServer[] = [];
	let beat: ReturnType<typeof setInterval> | undefined;
	const offs: (() => void)[] = [];

	const send = (to: string, kind: string, data: unknown) => api.post(`/calls/${id}/signal`, { from: peerId, to, kind, data }).catch(() => {});

	function remoteFor(peer: string, userId: string, name: string): Remote {
		const found = remotes.find((r) => r.peerId === peer);
		if (found) return found;
		const pc = new RTCPeerConnection({ iceServers: ice });
		const r: Remote = { peerId: peer, userId, name, pc, stream: null, state: 'connecting', pending: [] };
		if (local) for (const t of local.getTracks()) pc.addTrack(blur && t.kind === 'video' ? blur.track : t, local);
		else {
			pc.addTransceiver('audio', { direction: 'recvonly' });
			pc.addTransceiver('video', { direction: 'recvonly' });
		}
		pc.onicecandidate = (e) => e.candidate && send(peer, 'ice', e.candidate.toJSON());
		pc.ontrack = (e) => {
			const x = remotes.find((y) => y.peerId === peer);
			if (x) x.stream = e.streams[0] ?? new MediaStream([e.track]);
		};
		pc.onconnectionstatechange = () => {
			const x = remotes.find((y) => y.peerId === peer);
			if (x) x.state = pc.connectionState;
		};
		remotes.push(r);
		return remotes[remotes.length - 1];
	}
	async function call(p: { peer_id: string; user_id: string; name: string }) {
		const r = remoteFor(p.peer_id, p.user_id, p.name);
		const pc = r.pc!;
		const offer = await pc.createOffer();
		await pc.setLocalDescription(offer);
		send(p.peer_id, 'offer', offer);
	}
	async function flush(r: Remote) {
		for (const c of r.pending.splice(0)) await r.pc?.addIceCandidate(c).catch(() => {});
	}
	async function onSignal(d: { call_id: string; from: string; to: string; kind: string; data: any }) {
		if (d.call_id !== id || d.to !== peerId) return;
		if (d.kind === 'offer') {
			const who = (await api.get<{ in_call: { peer_id: string; user_id: string; name: string }[] }>(`/calls/${id}`)).in_call.find((x) => x.peer_id === d.from);
			const r = remoteFor(d.from, who?.user_id ?? '', who?.name ?? 'Someone');
			const pc = r.pc!;
			await pc.setRemoteDescription(d.data);
			const answer = await pc.createAnswer();
			await pc.setLocalDescription(answer);
			send(d.from, 'answer', answer);
			await flush(r);
		} else {
			const r = remotes.find((x) => x.peerId === d.from);
			if (!r?.pc) return;
			if (d.kind === 'answer') {
				await r.pc.setRemoteDescription(d.data);
				await flush(r);
			} else if (r.pc.remoteDescription) await r.pc.addIceCandidate(d.data).catch(() => {});
			else r.pending.push(d.data);
		}
	}
	function drop(peer: string) {
		const r = remotes.find((x) => x.peerId === peer);
		r?.pc?.close();
		remotes = remotes.filter((x) => x.peerId !== peer);
	}

	// ----- through LiveKit -----
	const streams = new Map<string, MediaStream>();
	async function connectLiveKit(lk: { url: string; token: string }) {
		const { Room, RoomEvent, Track } = await import('livekit-client');
		const r = new Room({ adaptiveStream: true, dynacast: true });
		room = r;
		// One tile per browser in the room: its screen share (or camera) and its sound.
		const sync = () => {
			remotes = Array.from(r.remoteParticipants.values()).map((p) => {
				const tracks = Array.from(p.trackPublications.values()).flatMap((pub) => (pub.track && pub.isSubscribed ? [pub.track] : []));
				const picture = tracks.find((t) => t.source === Track.Source.ScreenShare) ?? tracks.find((t) => t.source === Track.Source.Camera);
				const want = [...(picture ? [picture.mediaStreamTrack] : []), ...tracks.filter((t) => t.kind === 'audio').map((t) => t.mediaStreamTrack)];
				let ms = streams.get(p.identity);
				if (!ms) streams.set(p.identity, (ms = new MediaStream()));
				for (const t of ms.getTracks()) if (!want.includes(t)) ms.removeTrack(t);
				for (const t of want) if (!ms.getTracks().includes(t)) ms.addTrack(t);
				let userId = '';
				try {
					userId = JSON.parse(p.metadata ?? '{}').user_id ?? '';
				} catch {}
				return { peerId: p.identity, userId, name: p.name || 'Someone', pc: null, stream: ms, state: 'connected', pending: [] };
			});
		};
		r.on(RoomEvent.ParticipantConnected, sync)
			.on(RoomEvent.ParticipantDisconnected, (p) => (streams.delete(p.identity), sync()))
			.on(RoomEvent.TrackSubscribed, sync)
			.on(RoomEvent.TrackUnsubscribed, sync)
			.on(RoomEvent.Disconnected, () => status === 'in' && toasts.error('Lost the connection to the video server'));
		await r.connect(lk.url, lk.token);
		for (const t of local?.getTracks() ?? []) await r.localParticipant.publishTrack(t, { source: t.kind === 'audio' ? Track.Source.Microphone : Track.Source.Camera });
		sync();
	}

	// ----- recording -----
	let recBusy = $state(false);
	async function record() {
		recBusy = true;
		try {
			if (recording) {
				await api.del(`/calls/${id}/recording`);
				recording = null;
			} else recording = (await api.post<{ recording: Recording }>(`/calls/${id}/recording`)).recording;
		} catch (e) {
			toasts.error(e);
		} finally {
			recBusy = false;
		}
	}

	onMount(async () => {
		try {
			callInfo = await api.get(`/calls/${id}`);
			if (callInfo?.ended_at) return void (status = 'ended');
			try {
				local = await navigator.mediaDevices.getUserMedia({ audio: true, video: true });
				// Join with what the host doesn't allow switched off.
				if (callInfo && callInfo.host_id !== session.user?.id) {
					if (!callInfo.allow.audio) local.getAudioTracks().forEach((t) => (t.enabled = mic = false));
					if (!callInfo.allow.video) local.getVideoTracks().forEach((t) => (t.enabled = cam = false));
				}
			} catch {
				try {
					local = await navigator.mediaDevices.getUserMedia({ audio: true });
					cam = false;
				} catch {
					toasts.error('No camera or microphone: you can watch and listen only.');
				}
			}
			offs.push(realtime.on('call.signal', onSignal));
			offs.push(realtime.on('call.peer', (d: { call_id: string; event: string; peer_id: string }) => d.call_id === id && d.event === 'left' && drop(d.peer_id)));
			offs.push(realtime.on('call.declined', (d: { call_id: string; name: string }) => d.call_id === id && toasts.success(`${d.name} can't join now`)));
			offs.push(realtime.on('call.recording', (d: { call_id: string; recording: Recording }) => d.call_id === id && (recording = d.recording)));
			recording = callInfo?.recording ?? null;
			allow = callInfo?.allow ?? allow;
			offs.push(realtime.on('call.controls', onControls));
			const j = await api.post<{ peer_id: string; peers: { peer_id: string; user_id: string; name: string }[]; ice_servers?: RTCIceServer[]; livekit?: { url: string; token: string } }>(`/calls/${id}/join`, {});
			peerId = j.peer_id;
			if (j.livekit) {
				await connectLiveKit(j.livekit);
				status = 'in';
			} else {
				ice = j.ice_servers ?? [];
				status = 'in';
				for (const p of j.peers) await call(p);
			}
			beat = setInterval(() => api.post(`/calls/${id}/heartbeat`, { peer_id: peerId }).catch(() => (status = 'left')), 15000);
			window.addEventListener('pagehide', leaveQuietly);
		} catch (e) {
			status = 'failed';
			toasts.error(e);
		}
	});
	function leaveQuietly() {
		if (peerId && status === 'in') sendOnUnload(`/calls/${id}/leave`, { peer_id: peerId });
	}
	// ----- background blur -----
	let blur = $state<Blurred | null>(null);
	let blurBusy = $state(false);
	// What I see of myself: the blurred picture when blurring.
	const selfView = $derived(blur ? new MediaStream([blur.track]) : local);
	/** Sends this video track instead of the camera's (not while sharing the screen). */
	async function sendVideo(track: MediaStreamTrack) {
		if (room) {
			for (const pub of room.localParticipant.trackPublications.values()) {
				if (pub.source === 'camera' && pub.videoTrack) await pub.videoTrack.replaceTrack(track, true);
			}
		} else if (!sharing) {
			for (const r of remotes) await r.pc?.getSenders().find((x) => x.track?.kind === 'video')?.replaceTrack(track);
		}
	}
	async function toggleBlur() {
		const camera = local?.getVideoTracks()[0];
		if (!camera) return;
		blurBusy = true;
		try {
			if (blur) {
				await sendVideo(camera);
				blur.stop();
				blur = null;
			} else {
				const b = await blurTrack(camera);
				await sendVideo(b.track);
				blur = b;
			}
		} catch {
			toasts.error('Background blur is not available in this browser');
		} finally {
			blurBusy = false;
		}
	}

	function stopAll() {
		clearInterval(beat);
		blur?.stop();
		offs.forEach((f) => f());
		for (const r of remotes) r.pc?.close();
		room?.disconnect();
		room = null;
		local?.getTracks().forEach((t) => t.stop());
		sharing?.getTracks().forEach((t) => t.stop());
		window.removeEventListener('pagehide', leaveQuietly);
	}
	onDestroy(() => {
		leaveQuietly();
		stopAll();
	});
	async function hangUp() {
		if (peerId) await api.post(`/calls/${id}/leave`, { peer_id: peerId }).catch(() => {});
		status = 'left';
		stopAll();
		goto(callInfo?.chat_id ? `/messenger?chat=${callInfo.chat_id}` : '/');
	}
	function toggle(kind: 'audio' | 'video') {
		const on = kind === 'audio' ? (mic = !mic) : (cam = !cam);
		if (room) {
			// Others see it as muted rather than a black picture or silence.
			for (const pub of room.localParticipant.trackPublications.values()) {
				if (pub.kind === kind && pub.source !== 'screen_share') on ? pub.track?.unmute() : pub.track?.mute();
			}
		} else local?.getTracks().filter((t) => t.kind === kind).forEach((t) => (t.enabled = on));
	}
	async function share() {
		if (sharing) return stopShare();
		try {
			sharing = await navigator.mediaDevices.getDisplayMedia({ video: true });
			const track = sharing.getVideoTracks()[0];
			if (room) {
				const { Track } = await import('livekit-client');
				await room.localParticipant.publishTrack(track, { source: Track.Source.ScreenShare });
			} else for (const r of remotes) r.pc?.getSenders().find((s) => s.track?.kind === 'video')?.replaceTrack(track);
			track.onended = stopShare;
		} catch {
			sharing = null;
		}
	}
	function stopShare() {
		const track = sharing?.getVideoTracks()[0];
		if (room && track) room.localParticipant.unpublishTrack(track).catch(() => {});
		else {
			const cameraTrack = blur?.track ?? local?.getVideoTracks()[0] ?? null;
			for (const r of remotes) r.pc?.getSenders().find((s) => s.track?.kind === 'video')?.replaceTrack(cameraTrack);
		}
		sharing?.getTracks().forEach((t) => t.stop());
		sharing = null;
	}
	function video(node: HTMLVideoElement, stream: MediaStream | null) {
		node.srcObject = stream;
		return { update: (s: MediaStream | null) => node.srcObject !== s && (node.srcObject = s) };
	}
	const waitingFor = $derived(callInfo?.people.filter((p) => p.id !== session.user?.id && !remotes.some((r) => r.userId === p.id)) ?? []);
</script>

<svelte:head><title>Call · NERP</title></svelte:head>

<div class="flex h-[calc(100dvh-8rem)] min-h-[30rem] flex-col rounded-2xl bg-slate-900 p-4 text-white" data-call-status={status} data-call-media={callInfo?.media}>
	{#if recording && status === 'in'}
		<p class="mb-3 flex items-center gap-2 self-center rounded-full bg-red-600/90 px-3 py-1 text-sm font-medium" data-recording>
			<span class="size-2 animate-pulse rounded-full bg-white"></span>Recording{recording.started_by_name ? ` · started by ${recording.started_by_name}` : ''}
		</p>
	{/if}
	{#if status === 'ended'}
		<p class="m-auto text-slate-300">This call has ended.</p>
	{:else if status === 'failed'}
		<p class="m-auto text-slate-300">Couldn't join the call.</p>
	{:else}
		<div class="grid flex-1 auto-rows-fr gap-3 {remotes.length === 0 ? 'grid-cols-1' : remotes.length < 2 ? 'grid-cols-2' : 'grid-cols-2 lg:grid-cols-3'}">
			<div class="relative overflow-hidden rounded-xl bg-slate-800">
				<video use:video={sharing ?? selfView} autoplay playsinline muted class="size-full object-cover {sharing ? '' : '-scale-x-100'}"></video>
				<span class="absolute bottom-2 left-2 rounded bg-black/50 px-2 py-0.5 text-xs">You{mic ? '' : ' · muted'}</span>
			</div>
			{#each remotes as r (r.peerId)}
				<div class="relative overflow-hidden rounded-xl bg-slate-800" data-remote={r.name} data-peer-state={r.state}>
					<video use:video={r.stream} autoplay playsinline class="size-full object-cover"></video>
					<span class="absolute bottom-2 left-2 rounded bg-black/50 px-2 py-0.5 text-xs">{r.name}{r.state !== 'connected' ? ` · ${r.state}` : ''}</span>
				</div>
			{/each}
		</div>
		{#if waitingFor.length && status === 'in'}<p class="mt-3 text-center text-sm text-slate-400">Waiting for {waitingFor.map((p) => p.name).join(', ')}…</p>{/if}
		<div class="mt-4 flex justify-center gap-3">
			<button class="flex size-12 items-center justify-center rounded-full disabled:opacity-40 {mic ? 'bg-slate-700' : 'bg-red-600'}" aria-pressed={!mic} aria-label={mic ? 'Mute' : 'Unmute'} disabled={!mic && !allow.audio && !isHost} title={!allow.audio && !isHost ? 'The host turned microphones off' : ''} onclick={() => toggle('audio')}>{mic ? '🎙' : '🔇'}</button>
			<button class="flex size-12 items-center justify-center rounded-full disabled:opacity-40 {cam ? 'bg-slate-700' : 'bg-red-600'}" aria-pressed={!cam} aria-label={cam ? 'Turn camera off' : 'Turn camera on'} disabled={!cam && !allow.video && !isHost} title={!allow.video && !isHost ? 'The host turned cameras off' : ''} onclick={() => toggle('video')}><Icon name="monitor" class="size-5" /></button>
			<button class="flex size-12 items-center justify-center rounded-full text-xs disabled:opacity-40 {blur ? 'bg-brand-600' : 'bg-slate-700'}" aria-pressed={!!blur} aria-label="Blur background" title="Blur the background" disabled={blurBusy || !cam || !local?.getVideoTracks().length} onclick={toggleBlur} data-blur>{blurBusy ? '…' : 'Blur'}</button>
			<button class="flex size-12 items-center justify-center rounded-full disabled:opacity-40 {sharing ? 'bg-brand-600' : 'bg-slate-700'}" aria-pressed={!!sharing} aria-label="Share screen" disabled={!sharing && !allow.screen && !isHost} title={!allow.screen && !isHost ? 'The host turned screen sharing off' : ''} onclick={share}>⧉</button>
			{#if isHost && status === 'in'}
				<div class="relative">
					<button class="flex h-12 items-center rounded-full bg-slate-700 px-4 text-sm font-medium" aria-expanded={hostOpen} onclick={() => (hostOpen = !hostOpen)} data-host-controls>Host</button>
					{#if hostOpen}
						<div class="absolute bottom-14 left-1/2 w-64 -translate-x-1/2 space-y-2 rounded-xl bg-slate-800 p-3 text-sm shadow-xl" role="group" aria-label="Host controls">
							<button class="w-full rounded-lg bg-slate-700 px-3 py-2 text-left hover:bg-slate-600" onclick={() => setControls({ mute_all: true })}>🔇 Mute everyone</button>
							<label class="flex items-center gap-2"><input type="checkbox" checked={allow.audio} onchange={(e) => setControls({ allow_audio: e.currentTarget.checked })} /> Others may use their microphone</label>
							<label class="flex items-center gap-2"><input type="checkbox" checked={allow.video} onchange={(e) => setControls({ allow_video: e.currentTarget.checked })} /> Others may use their camera</label>
							<label class="flex items-center gap-2"><input type="checkbox" checked={allow.screen} onchange={(e) => setControls({ allow_screen: e.currentTarget.checked })} /> Others may share their screen</label>
						</div>
					{/if}
				</div>
			{/if}
			{#if callInfo?.can_record && status === 'in'}
				<button class="flex h-12 items-center gap-2 rounded-full px-5 font-medium {recording ? 'bg-red-600' : 'bg-slate-700'}" disabled={recBusy} aria-pressed={!!recording} onclick={record}>
					<span class="size-3 rounded-full {recording ? 'bg-white' : 'bg-red-500'}"></span>{recording ? 'Stop recording' : 'Record'}
				</button>
			{/if}
			<button class="flex h-12 items-center justify-center rounded-full bg-red-600 px-6 font-medium" onclick={hangUp}>Leave</button>
		</div>
	{/if}
</div>
