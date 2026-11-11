// ID-Control: Clipboard API may be unavailable inside the Bitrix24 desktop iframe.
// Use it first, fall back to a user-initiated legacy copy, then select for Ctrl+C.
(() => {
  'use strict';
  const input = document.getElementById('contractor-url');
  const button = document.getElementById('copy-contractor-url');
  const status = document.getElementById('contractor-copy-status');
  if (!input || !button || !status) return;

  try {
    input.value = new URL(input.dataset.path, window.location.href).href;
  } catch (_err) {
    status.textContent = 'Не удалось сформировать ссылку';
    button.disabled = true;
    return;
  }

  function selectLink() {
    input.focus();
    input.select();
    if (typeof input.setSelectionRange === 'function') {
      input.setSelectionRange(0, input.value.length);
    }
  }

  button.addEventListener('click', async () => {
    if (button.disabled || !input.value) return;
    button.disabled = true;
    try {
      if (navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
        try {
          await navigator.clipboard.writeText(input.value);
          status.textContent = 'Ссылка скопирована';
          return;
        } catch (_err) {
          // Desktop Bitrix24 may deny clipboard-write in its iframe.
        }
      }

      // execCommand is deprecated, but remains a useful fallback in Chromium
      // hosts that prohibit asynchronous Clipboard API inside an iframe.
      const helper = document.createElement('textarea');
      helper.value = input.value;
      helper.style.position = 'fixed';
      helper.style.left = '-9999px';
      helper.style.top = '0';
      document.body.appendChild(helper);
      helper.focus();
      helper.select();
      let copied = false;
      try {
        copied = typeof document.execCommand === 'function' && document.execCommand('copy') === true;
      } catch (_err) { /* user can always copy a selected input manually */ }
      helper.remove();
      if (copied) {
        status.textContent = 'Ссылка скопирована';
      } else {
        selectLink();
        status.textContent = 'Ссылка выделена. Нажмите Ctrl+C для копирования.';
      }
    } finally {
      button.disabled = false;
    }
  });
})();
