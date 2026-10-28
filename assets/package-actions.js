// ID-Control v0.9.18: reliable forms in the Bitrix24 desktop iframe.
// This is UI-only. All permissions and CSRF verification remain on the Rust server.
(() => {
  'use strict';
  const upload = document.querySelector('form[data-package-upload]');
  if (upload) {
    const button = upload.querySelector('[data-upload-button]');
    const feedback = upload.querySelector('[data-upload-feedback]');
    const progress = upload.querySelector('[data-upload-progress]');
    const picker = upload.querySelector('input[type=file][name=file]');
    const showMissingFile = () => {
      if (picker && (!picker.files || picker.files.length === 0) && feedback) {
        feedback.textContent = 'Сначала выберите файлы для загрузки.';
      }
    };
    if (button) button.addEventListener('click', showMissingFile);
    if (picker) picker.addEventListener('invalid', showMissingFile);
    upload.addEventListener('submit', event => {
      if (!picker || !picker.files || picker.files.length === 0) {
        event.preventDefault();
        if (feedback) feedback.textContent = 'Сначала выберите файлы для загрузки.';
        if (picker) picker.focus();
        return;
      }
      if (!window.XMLHttpRequest || !window.FormData) {
        // Native form submission remains available in older embedded engines.
        if (feedback) feedback.textContent = 'Загрузка началась…';
        return;
      }
      event.preventDefault();
      if (button.disabled) return;
      button.disabled = true;
      if (progress) { progress.hidden = false; progress.value = 0; }
      if (feedback) feedback.textContent = 'Передача файлов: 0%';
      const xhr = new XMLHttpRequest();
      xhr.open('POST', upload.action, true);
      xhr.withCredentials = true;
      xhr.upload.addEventListener('progress', e => {
        if (e.lengthComputable && e.total > 0) {
          const pct = Math.min(100, Math.round(e.loaded * 100 / e.total));
          if (progress) progress.value = pct;
          if (feedback) feedback.textContent = pct < 100
            ? `Передача файлов: ${pct}%`
            : 'Файлы переданы. Идёт регистрация и распознавание документов…';
        }
      });
      xhr.addEventListener('load', () => {
        button.disabled = false;
        if (xhr.status >= 200 && xhr.status < 300) {
          if (feedback) feedback.textContent = 'Документы обработаны. Обновляем список…';
          // POST normally returns 303; XHR follows it and responseURL is the package page.
          window.location.assign(xhr.responseURL || upload.action.replace(/\/upload$/, '?notice=uploaded#documents'));
        } else {
          const info = (xhr.responseText || '').replace(/<[^>]*>/g, ' ').trim().slice(0, 500);
          if (feedback) feedback.textContent = `Ошибка загрузки (HTTP ${xhr.status}): ${info || 'Смотрите журнал сервера.'}`;
        }
      });
      xhr.addEventListener('error', () => {
        button.disabled = false;
        if (feedback) feedback.textContent = 'Соединение прервано. Проверьте доступность приложения.';
      });
      xhr.addEventListener('abort', () => {
        button.disabled = false;
        if (feedback) feedback.textContent = 'Загрузка прервана.';
      });
      try { xhr.send(new FormData(upload)); }
      catch (err) { button.disabled = false; if (feedback) feedback.textContent = 'Не удалось начать отправку: ' + err.message; }
    });
  }

  const scan = document.querySelector('form[data-package-scan]');
  if (!scan || !window.fetch) return;
  const button = scan.querySelector('[data-scan-button]');
  const feedback = scan.querySelector('[data-scan-feedback]');
  let watch = null;
  let started = false;
  async function poll() {
    try {
      const response = await fetch(scan.dataset.statusUrl, {credentials:'same-origin', cache:'no-store'});
      if (!response.ok) throw Error('HTTP ' + response.status);
      const info = await response.json();
      if (feedback) feedback.textContent = info.message || (info.running ? 'Сканирование продолжается…' : 'Сканирование не выполняется.');
      if (button) button.disabled = !!info.running;
      if (!info.running && watch) {
        clearInterval(watch); watch = null;
        // Refresh the file list only when this page initiated the operation.
        if (started && !/ошибка/i.test(info.message || '')) window.location.reload();
        started = false;
      }
    } catch (err) {
      if (feedback) feedback.textContent = 'Не удалось получить статус сканирования: ' + err.message;
      if (button) button.disabled = false;
      if (watch) { clearInterval(watch); watch = null; }
    }
  }
  function watchScan() {
    if (!watch) watch = setInterval(poll, 2500);
    poll();
  }
  scan.addEventListener('submit', async event => {
    event.preventDefault();
    if (button && button.disabled) return;
    if (button) button.disabled = true;
    if (feedback) feedback.textContent = 'Запуск сканирования…';
    try {
      const response = await fetch(scan.action, {
        method: 'POST', credentials: 'same-origin', cache: 'no-store',
        headers: {'Content-Type':'application/x-www-form-urlencoded;charset=UTF-8','X-Requested-With':'XMLHttpRequest'},
        body: new URLSearchParams(new FormData(scan))
      });
      if (!response.ok) {
        const txt = (await response.text()).slice(0, 500);
        if (feedback) feedback.textContent = `Ошибка сканирования (HTTP ${response.status}): ${txt || 'Смотрите журнал сервера.'}`;
        if (response.status !== 409) { if (button) button.disabled = false; return; }
      } else {
        started = true;
      }
      watchScan();
    } catch (err) {
      if (button) button.disabled = false;
      if (feedback) feedback.textContent = 'Запрос не отправлен: ' + err.message;
    }
  });
  // Restore the visible status after returning to an already-open package page.
  poll();
})();
