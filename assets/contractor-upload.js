// Contractor ZIP upload v0.9.12: separate browser transfer and durable backend processing.
(() => {
  'use strict';
  const form = document.querySelector('form[action^="/contractor/"]');
  const processing = document.getElementById('delivery-progress');
  const sendStatus = document.getElementById('send-status');
  const sendBar = document.getElementById('send-bar');
  const label = document.getElementById('processing-stage');
  const bar = document.getElementById('processing-bar');
  const value = document.getElementById('processing-value');
  const detail = document.getElementById('processing-detail');
  let timer = null;
  let inFlight = false;
  let done = false;
  const stageLabels = {
    queued: 'Архив принят. Задание ожидает обработки',
    extracting: 'Распаковка ZIP на сервере',
    synology: 'Передача документов на Synology',
    publishing: 'Публикация версии комплекта на NAS',
    completed: 'Комплект успешно размещён на Synology',
    failed: 'Ошибка обработки архива',
  };
  function percent(n) { return Math.min(100, Math.max(0, Number(n) || 0)); }
  function renderStatus(data) {
    if (!processing) return;
    processing.hidden = false;
    // A registered submission must never expose a second upload action.
    if (form && data.receipt) form.hidden = true;
    const number = data.receipt ? ` · ${data.receipt}` : '';
    const stage = data.stage || 'queued';
    if (label) label.textContent = `${stageLabels[stage] || 'Обработка документов'}${number}`;
    if (bar) bar.value = percent(data.percent);
    if (value) value.textContent = `${percent(data.percent)} %`;
    if (detail) {
      const transferred = Number(data.bytes) || 0;
      const total = Number(data.total_bytes) || 0;
      const mb = (n) => (n / 1048576).toLocaleString('ru-RU', { maximumFractionDigits: 1 });
      detail.textContent = data.failed ? (data.message || 'Обратитесь к ответственному сотруднику.')
        : data.complete ? 'Поставка зарегистрирована. Теперь её проверяет ответственный сотрудник.'
        : total > 0 ? `${mb(Math.min(transferred, total))} из ${mb(total)} МБ в текущем этапе. Можно закрыть страницу: обработка продолжится.`
        : 'Задание выполняется на сервере. Страницу можно закрыть и открыть снова по этой ссылке.';
    }
    if (data.complete || data.failed) {
      done = true;
      if (timer) clearInterval(timer);
    }
  }
  async function poll() {
    if (!processing || inFlight || done) return;
    inFlight = true;
    try {
      const response = await fetch(processing.dataset.statusUrl, { cache: 'no-store', credentials: 'omit' });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      renderStatus(await response.json());
    } catch (err) {
      if (detail) detail.textContent = `Связь с сервером временно отсутствует (${String(err)}). Продолжим проверять статус.`;
    } finally {
      inFlight = false;
    }
  }
  function startPolling() {
    if (!processing || timer || done) return;
    processing.hidden = false;
    void poll();
    timer = setInterval(poll, 1500);
  }
  // A claimed link has no form but still renders the status view on refresh.
  if (!form) { if (processing) startPolling(); return; }
  form.addEventListener('submit', (event) => {
    event.preventDefault();
    const button = form.querySelector('button[type="submit"]');
    const file = form.querySelector('input[name="archive"]');
    if (button?.disabled || !file?.files?.length) return;
    if (button) button.disabled = true;
    if (sendStatus) sendStatus.textContent = 'Передаём ZIP на сервер: 0 %';
    if (sendBar) { sendBar.hidden = false; sendBar.value = 0; }
    const xhr = new XMLHttpRequest();
    xhr.open('POST', form.action, true);
    xhr.setRequestHeader('Accept', 'application/json');
    xhr.upload.onprogress = (event) => {
      if (!event.lengthComputable) return;
      const pct = percent(Math.round(event.loaded * 100 / event.total));
      if (sendStatus) sendStatus.textContent = `Передача ZIP на сервер: ${pct} %`;
      if (sendBar) sendBar.value = pct;
    };
    xhr.upload.onload = () => {
      if (sendStatus) sendStatus.textContent = 'Архив отправлен. Регистрируем поставку…';
    };
    xhr.onload = () => {
      let result;
      try { result = JSON.parse(xhr.responseText); } catch (_) { result = {}; }
      if (xhr.status === 202 && result.accepted) {
        if (sendStatus) sendStatus.textContent = `ZIP принят сервером. Регистрационный номер: ${result.receipt}`;
        if (form) form.hidden = true;
        if (processing && result.status_url) processing.dataset.statusUrl = result.status_url;
        startPolling();
      } else {
        if (sendStatus) sendStatus.textContent = result.error || `Ошибка приёма ZIP (HTTP ${xhr.status})`;
        if (button) button.disabled = false;
      }
    };
    xhr.onerror = () => {
      if (sendStatus) sendStatus.textContent = 'Связь прервана. Проверьте статус по этой же ссылке перед повторной попыткой.';
      // A request might have been accepted despite a broken client connection.
      startPolling();
    };
    xhr.send(new FormData(form));
  });
})();
