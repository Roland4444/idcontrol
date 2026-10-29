// v0.9.30: semantic status — one source of truth for every on-screen job row.
// Installed once at the end of the fully rendered package page, not inside
// an HTML fragment that may be replaced by another panel.
(() => {
  'use strict';
  const refreshButton = document.querySelector('[data-semantic-refresh]');
  const jobRows = document.getElementById('semantic-job-rows');
  const info = document.getElementById('semantic-progress');
  if (!refreshButton || !jobRows || !info) return;

  let busy = false;
  const label = kind => kind === 'extract' ? 'Извлечение' : kind === 'compare' ? 'Сопоставление' : kind;
  const stateLabel = state => ({running:'Выполняется', pending:'В очереди', ready:'Завершено', error:'Ошибка'})[state] || state;
  const int = (v, fallback=0) => Number.isFinite(Number(v)) ? Math.max(0, Math.trunc(Number(v))) : fallback;
  const minutes = v => `${Math.floor(int(v) / 60)} мин`;
  const td = (row, value) => {
    const cell = document.createElement('td');
    cell.textContent = String(value == null ? '' : value);
    row.appendChild(cell);
    return cell;
  };

  function updateJobs(jobs) {
    // Show the newest job first. Do not replace it with the last failed job.
    const fragment = document.createDocumentFragment();
    const all = Array.isArray(jobs) ? jobs : [];
    if (all.length === 0) {
      const row = document.createElement('tr');
      const cell = td(row, 'Заданий ещё нет.');
      cell.colSpan = 4;
      fragment.appendChild(row);
    }
    all.forEach(job => {
      const row = document.createElement('tr');
      row.dataset.semanticJobId = String(job.id);
      const done = int(job.completed);
      const total = int(job.total);
      const status = job.status || 'unknown';
      td(row, `№${job.id} · ${label(job.kind)}`);
      td(row, job.model || '—');
      const progCell = td(row, `${done}/${total || '—'} · найдено: ${int(job.found)}`);
      if (total > 0) {
        const prog = document.createElement('progress');
        prog.max = total;
        prog.value = Math.min(done,total);
        prog.setAttribute('aria-label', `Обработка ${done} из ${total}`);
        prog.style.display = 'block';
        prog.style.maxWidth = '200px';
        progCell.appendChild(prog);
      }
      const details = [];
      if (status === 'error' && job.error) details.push(job.error);
      else if (job.stage) details.push(job.stage);
      else if (status === 'running') details.push('Выполняется запрос к модели; ожидается следующий завершённый фрагмент');
      if (status === 'running') {
        details.push(`время задания: ${minutes(job.elapsed_seconds)}`);
        if (job.stage) details.push(`от начала этапа: ${minutes(job.activity_age_seconds)}`);
      }
      td(row, `${stateLabel(status)}${details.length ? ' · ' + details.join(' · ') : ''}`);
      fragment.appendChild(row);
    });
    jobRows.replaceChildren(fragment);
    const running = all.filter(x => x.status === 'running' || x.status === 'pending');
    const current = running[0] || all[0];
    const summary = current ? `№${current.id}: ${stateLabel(current.status)} · ${int(current.completed)}/${int(current.total)}` : 'Заданий нет';
    info.textContent = `${summary} · Обновлено ${new Date().toLocaleTimeString('ru-RU')}`;
  }

  async function refresh(manual=false) {
    if (busy) {
      if (manual) info.textContent = 'Запрос обновления уже выполняется…';
      return;
    }
    if (!manual && document.hidden) return;
    busy = true;
    refreshButton.disabled = true;
    if (manual) info.textContent = 'Запрашиваю актуальные данные из PostgreSQL…';
    try {
      const response = await fetch(refreshButton.dataset.url, {
        credentials: 'same-origin',
        cache: 'no-store',
        headers: {'Accept':'application/json'}
      });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const result = await response.json();
      updateJobs(result.jobs);
      const evidence = document.getElementById('semantic-evidence-rows');
      if (evidence && typeof result.rows_html === 'string') evidence.innerHTML = result.rows_html;
    } catch (error) {
      info.textContent = `Ошибка обновления: ${String(error)}. Проверка в фоне может продолжаться.`;
    } finally {
      busy = false;
      refreshButton.disabled = false;
    }
  }

  refreshButton.addEventListener('click', () => void refresh(true));
  // Auto-refresh uses exactly the same path as the manual button.
  setInterval(() => void refresh(false), 5000);
  void refresh(true);
})();
