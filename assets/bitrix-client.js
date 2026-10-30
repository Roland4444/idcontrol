// Client bootstrap for Bitrix24. Inspired by BX24.init + user.current example.
// The login/email obtained in JavaScript is ONLY for display. Authentication
// is completed by the Rust server calling user.current independently.
(function () {
  'use strict';
  var status = document.getElementById('status');
  var help = document.getElementById('help');
  var stopped = false;
  var timeout = null;

  function fail(message) {
    if (stopped) return;
    stopped = true;
    if (timeout) clearTimeout(timeout);
    status.textContent = message;
    help.textContent = 'Откройте приложение в Bitrix24. Если вход не завершается, проверьте настройки iframe и cookie для сайта приложения.';
  }

  function redirectToIDControl(auth) {
    // Never trust a user ID, email or login supplied by browser JavaScript.
    // Rust checks the OAuth token against the configured Bitrix24 portal.
    fetch('/auth/bitrix', {
      method: 'POST', credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ domain: auth.domain, access_token: auth.access_token })
    }).then(function (response) {
      if (!response.ok) throw new Error('HTTP ' + response.status);
      return response.json();
    }).then(function (data) {
      if (!data.ok) throw new Error('Bitrix24 не подтвердил личность');
      if (stopped) return;
      stopped = true;
      clearTimeout(timeout);
      // Stay inside the Bitrix24 iframe, now backed by a verified server cookie.
      window.location.replace('/');
    }).catch(function (err) { fail('Ошибка входа: ' + err.message); });
  }

  if (window.self === window.top) { fail('Запустите приложение из меню Bitrix24.'); return; }
  if (typeof BX24 === 'undefined') { fail('Библиотека Bitrix24 не загрузилась.'); return; }
  timeout = setTimeout(function () { fail('Нет ответа от Bitrix24.'); }, 15000);

  // One shared entrypoint for initial app installation and regular launches.
  // During installation installFinish completes the portal-wide setup.
  // Bitrix24 ignores this call on ordinary launches of an installed app.
  BX24.init(function () {
    if (stopped) return;
    BX24.installFinish();
    BX24.callMethod('user.current', {}, function (result) {
      if (stopped) return;
      if (result.error()) { fail('Bitrix24 не вернул текущего сотрудника.'); return; }
      var user = result.data() || {};
      var login = user.EMAIL || user.PERSONAL_PHONE || user.PHONE || user.LOGIN || user.ID;
      status.textContent = 'Проверяем доступ сотрудника: ' + (user.NAME || login || 'Bitrix24') + '…';
      var auth = BX24.getAuth();
      if (!auth || !auth.access_token || !auth.domain) {
        fail('Нет действующих данных авторизации Bitrix24. Переоткройте приложение.');
        return;
      }
      redirectToIDControl(auth);
    });
  });
})();
