/* NERP website tracking: <script src="https://crm.example.com/track.js" data-site="KEY" async></script>
   Keeps a visitor id in this site's storage and reports each page (address, title, where the
   visitor came from). NERP web forms and live chat on the same page pass the id along, so the
   CRM record they make shows the visitor's journey. Browsers asking not to be tracked
   (Global Privacy Control, Do Not Track) are left alone. */
(function () {
	var s = document.currentScript;
	if (!s || !s.dataset.site) return;
	if (navigator.globalPrivacyControl || navigator.doNotTrack === '1' || window.doNotTrack === '1') return;
	var url = new URL(s.src).origin + '/api/public/track/' + encodeURIComponent(s.dataset.site);
	var vid = null;
	try {
		vid = localStorage.getItem('nerp_vid');
	} catch (e) {}
	if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(vid || '')) {
		vid = crypto.randomUUID
			? crypto.randomUUID()
			: '10000000-1000-4000-8000-100000000000'.replace(/[018]/g, function (c) {
					return (c ^ (crypto.getRandomValues(new Uint8Array(1))[0] & (15 >> (c / 4)))).toString(16);
				});
		try {
			localStorage.setItem('nerp_vid', vid);
		} catch (e) {}
	}
	window.NERP_VID = vid;

	function send(referrer) {
		var body = JSON.stringify({ v: vid, u: location.href, t: document.title, r: referrer });
		try {
			if (navigator.sendBeacon && navigator.sendBeacon(url, new Blob([body], { type: 'text/plain' }))) return;
		} catch (e) {}
		try {
			fetch(url, { method: 'POST', body: body, mode: 'no-cors', keepalive: true, headers: { 'content-type': 'text/plain' } });
		} catch (e) {}
	}
	send(document.referrer);

	// Pages of single-page sites change without loading.
	var last = location.href;
	function changed() {
		if (location.href === last) return;
		var from = last;
		last = location.href;
		setTimeout(function () {
			send(from);
		}, 50);
	}
	['pushState', 'replaceState'].forEach(function (m) {
		var orig = history[m];
		history[m] = function () {
			var r = orig.apply(this, arguments);
			changed();
			return r;
		};
	});
	window.addEventListener('popstate', changed);
})();
