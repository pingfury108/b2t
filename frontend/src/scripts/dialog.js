// 对话框工具：alert / confirm / prompt 的 Promise 化替代
// alertDialog(msg)            -> Promise<void>
// confirmDialog(msg)          -> Promise<boolean>
// promptDialog(msg, initial)  -> Promise<string | null>

let styleInjected = false;

function injectStyle() {
  if (styleInjected) return;
  styleInjected = true;
  const style = document.createElement('style');
  style.textContent = `
    .dlg-mask {
      position: fixed; inset: 0; z-index: 100;
      background: var(--overlay, rgba(0, 0, 0, .3));
      display: flex; align-items: center; justify-content: center;
    }
    .dlg {
      background: var(--card, #fff); color: var(--ink, #1c1a17);
      border: 1px solid var(--line, #e8e2d9); border-radius: 10px;
      box-shadow: 0 8px 30px var(--shadow, rgba(0, 0, 0, .12));
      width: min(400px, 90vw); padding: 24px;
      font-family: inherit;
    }
    .dlg-msg { line-height: 1.7; margin-bottom: 18px; word-break: break-word; }
    .dlg-input {
      width: 100%; box-sizing: border-box; margin-bottom: 18px;
      padding: 10px 14px; font-size: 15px; font-family: inherit;
      border: 1px solid var(--line, #e8e2d9); border-radius: 6px;
      background: var(--bg, #faf8f4); color: inherit;
    }
    .dlg-input:focus { outline: none; border-color: var(--accent, #b3541e); }
    .dlg-btns { display: flex; justify-content: flex-end; gap: 10px; }
    .dlg-btn {
      padding: 8px 20px; border-radius: 6px; font-size: 15px;
      font-family: inherit; cursor: pointer;
      background: transparent; color: var(--accent, #b3541e);
      border: 1px solid var(--accent, #b3541e);
    }
    .dlg-btn.primary { background: var(--accent, #b3541e); color: #fff; }
  `;
  document.head.appendChild(style);
}

function open({ message, inputValue, onResolve }) {
  injectStyle();
  const mask = document.createElement('div');
  mask.className = 'dlg-mask';
  mask.innerHTML = `
    <div class="dlg" role="dialog" aria-modal="true">
      <p class="dlg-msg"></p>
      ${inputValue !== undefined ? '<input class="dlg-input" type="text" />' : ''}
      <div class="dlg-btns"></div>
    </div>`;

  const msgEl = mask.querySelector('.dlg-msg');
  msgEl.textContent = message;
  const inputEl = mask.querySelector('.dlg-input');
  if (inputEl) inputEl.value = inputValue;

  const close = (result) => {
    document.removeEventListener('keydown', onKey, true);
    mask.remove();
    onResolve(result);
  };
  const onKey = (e) => {
    if (e.key === 'Escape') { e.stopPropagation(); close(undefined); }
    if (e.key === 'Enter' && inputEl) { e.stopPropagation(); close(true); }
  };
  document.addEventListener('keydown', onKey, true);

  return { mask, inputEl, close };
}

export function alertDialog(message) {
  return new Promise((resolve) => {
    const { mask, close } = open({ message, onResolve: () => resolve() });
    const btns = mask.querySelector('.dlg-btns');
    const ok = document.createElement('button');
    ok.className = 'dlg-btn primary';
    ok.textContent = '确定';
    ok.addEventListener('click', () => close());
    btns.appendChild(ok);
    mask.addEventListener('click', (e) => { if (e.target === mask) close(); });
    document.body.appendChild(mask);
    ok.focus();
  });
}

export function confirmDialog(message) {
  return new Promise((resolve) => {
    const { mask, close } = open({
      message,
      onResolve: (r) => resolve(r === true),
    });
    const btns = mask.querySelector('.dlg-btns');
    const cancel = document.createElement('button');
    cancel.className = 'dlg-btn';
    cancel.textContent = '取消';
    cancel.addEventListener('click', () => close(false));
    const ok = document.createElement('button');
    ok.className = 'dlg-btn primary';
    ok.textContent = '确定';
    ok.addEventListener('click', () => close(true));
    btns.append(cancel, ok);
    document.body.appendChild(mask);
    ok.focus();
  });
}

export function promptDialog(message, initial = '') {
  return new Promise((resolve) => {
    const { mask, inputEl, close } = open({
      message,
      inputValue: initial,
      onResolve: (r) => resolve(r === true ? inputEl.value : null),
    });
    const btns = mask.querySelector('.dlg-btns');
    const cancel = document.createElement('button');
    cancel.className = 'dlg-btn';
    cancel.textContent = '取消';
    cancel.addEventListener('click', () => close(false));
    const ok = document.createElement('button');
    ok.className = 'dlg-btn primary';
    ok.textContent = '确定';
    ok.addEventListener('click', () => close(true));
    btns.append(cancel, ok);
    document.body.appendChild(mask);
    inputEl.focus();
    inputEl.select();
  });
}
