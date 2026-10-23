/* ID-Control: searchable employee picker. Bitrix24 is the source of truth.
   Only ACTIVE=true and USER_TYPE=employee are assignable inside the portal.
   The local debug mode can use the previously known local user directory. */
(function () {
  'use strict';
  const originalSelects = Array.from(document.querySelectorAll('select.employee-picker'));
  if (!originalSelects.length) return;

  const employees = new Map();
  const controls = [];
  let loading = true;
  let loadError = '';
  let debugDirectory = false;

  function isActive(value) {
    return value === true || value === 'Y' || value === 'true' || value === 1;
  }
  function isEligible(user) {
    return user && isActive(user.ACTIVE) && String(user.USER_TYPE || '').toLowerCase() === 'employee';
  }
  function fullname(user) {
    return [user.LAST_NAME, user.NAME, user.SECOND_NAME].map(v => String(v || '').trim()).filter(Boolean).join(' ');
  }
  function add(id, name) {
    id = String(id || '').trim();
    name = String(name || '').trim();
    if (!/^\d+$/.test(id) || Number(id) < 1 || !name) return;
    employees.set(id, {id, name, search: name.toLocaleLowerCase('ru-RU')});
    document.querySelectorAll('[data-employee-id]').forEach(el => {
      if (el.dataset.employeeId === id) el.textContent = name;
    });
  }
  const allEmployees = () => Array.from(employees.values()).sort((a,b) => a.name.localeCompare(b.name, 'ru'));
  function matches(user, query) {
    const terms = query.toLocaleLowerCase('ru-RU').trim().split(/\s+/).filter(Boolean);
    return terms.every(term => user.search.includes(term));
  }

  function makePicker(select) {
    // Keep the original select for a stable name=user_id in the existing Rust handlers.
    // No raw numeric IDs are presented in the interface.
    select.required = false;
    select.hidden = true;
    select.value = '';
    const wrapper = document.createElement('div');
    wrapper.className = 'employee-lookup';
    const input = document.createElement('input');
    input.type = 'search';
    input.className = 'employee-search';
    input.placeholder = 'Поиск по фамилии или имени…';
    input.autocomplete = 'off';
    input.spellcheck = false;
    input.required = true;
    input.setAttribute('role', 'combobox');
    input.setAttribute('aria-autocomplete', 'list');
    input.setAttribute('aria-expanded', 'false');
    input.setAttribute('aria-label', 'Поиск сотрудника по фамилии или имени');
    const list = document.createElement('div');
    list.className = 'employee-results';
    list.setAttribute('role', 'listbox');
    list.hidden = true;
    const status = document.createElement('small');
    status.className = 'employee-status';
    status.setAttribute('aria-live', 'polite');
    wrapper.append(input, list, status);
    select.insertAdjacentElement('afterend', wrapper);
    let filtered = [];
    let highlighted = -1;

    function clearSelection() {
      if (select.value) {
        select.value = '';
      }
      input.setCustomValidity('');
    }
    function choose(item) {
      let opt = Array.from(select.options).find(o => o.value === item.id);
      if (!opt) {opt = document.createElement('option'); opt.value = item.id; select.appendChild(opt);}
      opt.textContent = item.name;
      select.value = item.id;
      input.value = item.name;
      input.setCustomValidity('');
      close();
    }
    function close() {list.hidden = true; input.setAttribute('aria-expanded', 'false'); highlighted = -1;}
    function show() {
      list.replaceChildren();
      const q = input.value.trim();
      filtered = allEmployees().filter(u => matches(u, q)).slice(0, 40);
      const count = allEmployees().filter(u => matches(u, q)).length;
      if (loading) status.textContent = 'Загружаем сотрудников Bitrix24…';
      else if (loadError) status.textContent = loadError;
      else if (debugDirectory) status.textContent = 'Отладка: список ранее известных пользователей. Актуальный статус проверяется только в Bitrix24.';
      else status.textContent = count ? `Найдено: ${count}${count > 40 ? ' · показаны первые 40, уточните поиск' : ''}` : 'Активные сотрудники не найдены';
      filtered.forEach((person, i) => {
        const option = document.createElement('button');
        option.type = 'button';
        option.className = 'employee-result';
        option.textContent = person.name;
        option.setAttribute('role', 'option');
        option.setAttribute('aria-selected', i === highlighted ? 'true' : 'false');
        // Prevent blur from closing the popup before a mouse selection.
        option.addEventListener('mousedown', e => e.preventDefault());
        option.addEventListener('click', () => choose(person));
        list.appendChild(option);
      });
      list.hidden = false;
      input.setAttribute('aria-expanded', 'true');
    }
    input.addEventListener('input', () => {clearSelection(); highlighted = -1; show();});
    input.addEventListener('focus', show);
    input.addEventListener('keydown', e => {
      if (e.key === 'Escape') {close(); return;}
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        if (list.hidden) show();
        highlighted = Math.min(filtered.length - 1, Math.max(0, highlighted + (e.key === 'ArrowDown' ? 1 : -1)));
        Array.from(list.children).forEach((btn,i) => {btn.setAttribute('aria-selected', String(i === highlighted));});
        list.children[highlighted]?.scrollIntoView({block:'nearest'});
      } else if (e.key === 'Enter' && !list.hidden && highlighted >= 0 && filtered[highlighted]) {
        e.preventDefault(); choose(filtered[highlighted]);
      }
    });
    input.addEventListener('blur', () => setTimeout(close, 100));
    select.form?.addEventListener('submit', e => {
      if (!select.value || !employees.has(select.value)) {
        e.preventDefault();
        input.setCustomValidity('Выберите сотрудника из списка, а не просто введите имя');
        input.reportValidity();
        input.focus();
        show();
      } else if (debugDirectory === false && (loading || loadError)) {
        e.preventDefault();
        input.setCustomValidity('Список сотрудников Bitrix24 ещё не загружен');
        input.reportValidity();
      } else if (!debugDirectory) {
        // The Rust server verifies ACTIVE+employee again before any grant.
        const auth = typeof BX24 !== 'undefined' && typeof BX24.getAuth === 'function' ? BX24.getAuth() : null;
        if (!auth || !auth.access_token) {
          e.preventDefault();
          input.setCustomValidity('Обновите приложение в Bitrix24: истекла авторизация');
          input.reportValidity();
          return;
        }
        let hidden = select.form.elements.namedItem('bitrix_token');
        if (!hidden) {
          hidden = document.createElement('input');
          hidden.type = 'hidden';
          hidden.name = 'bitrix_token';
          select.form.appendChild(hidden);
        }
        hidden.value = auth.access_token;
      }
    });
    return {refresh() {if (!list.hidden) show();}};
  }
  originalSelects.forEach(s => controls.push(makePicker(s)));
  const refresh = () => controls.forEach(c => c.refresh());

  function loadFromBitrix() {
    // The filter protects REST traffic; a second check below protects the UI
    // when a portal returns unfiltered or inconsistent records.
    BX24.callMethod('user.get', {FILTER: {ACTIVE: true, USER_TYPE: 'employee'}}, function (result) {
      if (result.error()) {
        loadError = 'Не удалось получить сотрудников Bitrix24. Проверьте право REST «user».';
        loading = false; refresh(); return;
      }
      (result.data() || []).filter(isEligible).forEach(u => add(u.ID, fullname(u)));
      if (result.more()) {
        const next = result.next();
        if (next === false) {loading = false; refresh();}
      } else {loading = false; refresh();}
    });
  }

  if (typeof BX24 !== 'undefined' && window.self !== window.top) {
    BX24.init(loadFromBitrix);
  } else {
    // A debug launch has no live Bitrix24 token. We must never mix the local
    // cache with a production Bitrix24 employee list.
    fetch('/api/me', {credentials:'same-origin'})
      .then(r => r.ok ? r.json() : null)
      .then(me => {
        if (!me || !me.debug) {
          loadError = 'Откройте раздел внутри Bitrix24 для выбора активных сотрудников.';
          loading = false; refresh(); return;
        }
        debugDirectory = true;
        return fetch('/api/employees/known', {credentials:'same-origin'})
          .then(r => r.ok ? r.json() : [])
          .then(users => {users.forEach(u => add(u.user_id, u.name)); loading = false; refresh();});
      }).catch(() => {loadError = 'Не удалось загрузить справочник сотрудников.';loading = false;refresh();});
  }
})();
