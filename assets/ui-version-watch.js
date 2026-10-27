// ID-Control UI updater for Bitrix24 desktop, browser iframe and debug mode.
// HTML is never cacheable; this watcher checks the server for a newer bundle.
(function () {
  'use strict';
  var keys = ['html', 'js', 'css', 'images'];
  var current = {};
  for (var i = 0; i < keys.length; i++) {
    var el = document.querySelector('meta[name="idcontrol-' + keys[i] + '-id"]');
    if (!el || !el.content) return;
    current[keys[i]] = el.content;
  }
  var checking = false;
  var updated = false;
  var reloadKey = '__ui_rev';

  function refresh(data) {
    if (updated) return;
    updated = true;
    // Use a new URL to defeat the cache in embedded Chromium/Bitrix24.
    var url = new URL(window.location.href);
    url.searchParams.set(reloadKey, data.html + '.' + data.js + '.' + data.css + '.' + data.images);
    window.location.replace(url.toString());
  }

  function check() {
    if (checking || updated || document.hidden) return;
    checking = true;
    fetch('/api/ui-version?_=' + Date.now(), {
      method: 'GET', credentials: 'same-origin', cache: 'no-store',
      headers: { 'Accept': 'application/json', 'Cache-Control': 'no-cache' }
    }).then(function (res) {
      if (!res.ok) throw new Error('HTTP ' + res.status);
      return res.json();
    }).then(function (data) {
      for (var i = 0; i < keys.length; i++) {
        var key = keys[i];
        if (typeof data[key] !== 'string') throw new Error('Unknown UI manifest');
        if (data[key] !== current[key]) { refresh(data); return; }
      }
    }).catch(function () {
      // A restart/network outage is not a new UI release. Retry later.
    }).finally(function () { checking = false; });
  }

  check();
  setInterval(check, 15000);
  document.addEventListener('visibilitychange', function () { if (!document.hidden) check(); });
  window.addEventListener('focus', check);
  window.addEventListener('pageshow', check);
})();
