(function () {
  'use strict';
  var status = document.getElementById('status');
  var help = document.getElementById('help');
  var finished = false;
  function fail(message) {
    if (finished) return;
    finished = true;
    status.textContent = message;
    help.textContent = 'Откройте приложение внутри Битрикс24. При блокировке cookies разрешите сторонние cookies для сайта приложения.';
  }
  if (window.self === window.top) { fail('Вход доступен только из Битрикс24.'); return; }
  if (typeof BX24 === 'undefined') { fail('Не загрузилась библиотека Битрикс24.'); return; }
  var timer = setTimeout(function () { fail('Нет ответа от Битрикс24.'); }, 15000);
  // Finish installation when Bitrix24 enters installation mode, not on every launch.
  BX24.install(function () { BX24.installFinish(); });
  BX24.init(function () {
    if (finished) return;
    BX24.callMethod('user.current', {}, function (result) {
      if (finished) return;
      clearTimeout(timer);
      if (result.error()) { fail('Битрикс24 не определил текущего сотрудника.'); return; }
      var user = result.data();
      status.textContent = 'Вход: ' + (user.NAME || user.ID) + '…';
      // getAuth() is supplied automatically by the BX24 SDK; the operator
      // does not obtain, copy, store, or refresh these credentials manually.
      var auth = BX24.getAuth();
      if (!auth || !auth.access_token || !auth.domain) {
        fail('Нет действующей сессии Битрикс24. Переоткройте приложение.');
        return;
      }
      fetch('/auth/bitrix', {
        method: 'POST',
        credentials: 'same-origin',
        headers: {'Content-Type': 'application/json'},
        // Do NOT transmit user.ID: Rust calls Bitrix24 user.current itself.
        body: JSON.stringify({domain: auth.domain, access_token: auth.access_token})
      }).then(function (reply) {
        if (!reply.ok) throw new Error('HTTP ' + reply.status);
        return reply.json();
      }).then(function (data) {
        if (!data.ok) throw new Error('Сервер отклонил авторизацию');
        finished = true;
        window.location.replace('/');
      }).catch(function (err) { fail('Ошибка входа: ' + err.message); });
    });
  });
})();
