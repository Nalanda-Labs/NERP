/* Embeds a NERP web form: <script src="https://crm.example.com/embed.js" data-form="KEY" async></script>
   Creates an iframe of the hosted form, passes the page address (for UTM tags) and follows its height. */
(function () {
	var s = document.currentScript;
	if (!s || !s.dataset.form) return;
	var key = s.dataset.form;
	var origin = new URL(s.src).origin;
	var f = document.createElement('iframe');
	var q = new URLSearchParams({ embed: '1', u: location.href, r: document.referrer });
	f.src = origin + '/f/' + encodeURIComponent(key) + '?' + q.toString();
	f.title = s.dataset.title || 'Contact form';
	f.loading = 'lazy';
	f.style.cssText = 'width:100%;max-width:560px;border:0;min-height:320px;display:block;background:transparent';
	s.parentNode.insertBefore(f, s);
	window.addEventListener('message', function (e) {
		if (e.origin !== origin || !e.data || e.data.nerpForm !== key) return;
		if (e.data.height) f.style.height = e.data.height + 'px';
		if (e.data.redirect && /^https?:\/\//.test(e.data.redirect)) location.href = e.data.redirect;
	});
})();
