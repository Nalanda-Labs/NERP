/* NERP live chat button: <script src="https://crm.example.com/chat.js" data-chat="KEY" async></script>
   Adds a round button in the corner that opens the chat in a small panel. */
(function () {
	var s = document.currentScript;
	if (!s || !s.dataset.chat) return;
	var key = s.dataset.chat;
	var origin = new URL(s.src).origin;
	var color = /^#[0-9a-f]{6}$/i.test(s.dataset.color || '') ? s.dataset.color : '#1a73e8';
	var open = false;
	var frame = null;

	var btn = document.createElement('button');
	btn.type = 'button';
	btn.setAttribute('aria-label', 'Chat with us');
	btn.style.cssText = 'position:fixed;right:20px;bottom:20px;z-index:2147483000;width:56px;height:56px;border-radius:50%;border:0;cursor:pointer;' +
		'background:' + color + ';color:#fff;box-shadow:0 4px 14px rgba(0,0,0,.25);display:flex;align-items:center;justify-content:center';
	var icon = '<svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.4A8 8 0 1 1 21 12z"/></svg>';
	var close = '<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>';
	btn.innerHTML = icon;

	function toggle() {
		open = !open;
		if (open && !frame) {
			frame = document.createElement('iframe');
			var q = new URLSearchParams({ u: location.href });
			frame.src = origin + '/c/' + encodeURIComponent(key) + '?' + q.toString();
			frame.title = 'Chat';
			frame.style.cssText = 'position:fixed;right:20px;bottom:88px;z-index:2147483000;width:min(380px,calc(100vw - 40px));height:min(560px,calc(100vh - 120px));' +
				'border:0;border-radius:16px;box-shadow:0 8px 30px rgba(0,0,0,.25);background:#fff';
			document.body.appendChild(frame);
		}
		if (frame) frame.style.display = open ? 'block' : 'none';
		btn.innerHTML = open ? close : icon;
		btn.setAttribute('aria-label', open ? 'Close chat' : 'Chat with us');
	}
	btn.addEventListener('click', toggle);
	document.body.appendChild(btn);
})();
