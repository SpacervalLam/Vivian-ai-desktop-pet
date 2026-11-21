/**
 * Packaged page actions, instantiated per agent session in a CDP isolated world.
 *
 * 在受控标签页内运行（每份文档一个实例），把页面渲染成结构化文本快照，并按
 * 稳定编号在真实页面里执行点击 / 输入 / 按键 / 滚动 / 导航等动作，保留登录态。
 * 脚本负责观察与输入前检查；鼠标和文字输入由宿主通过 CDP Input 发送。
 *
 * 能力面：extract（元素提取）/ ids（稳定编号）/ snapshot（结构化快照）/
 * actions（页面动作）/ privacy（敏感字段掩码）。
 */

globalThis.vivianCreatePage = () => {
  'use strict';

  // ── extract ───────────────────────────────────────────────

  const INTERACTIVE_SELECTOR = [
    'a[href]',
    'button',
    'input:not([type="hidden"])',
    'select',
    'textarea',
    '[role="button"]',
    '[role="link"]',
    '[role="checkbox"]',
    '[role="radio"]',
    '[role="tab"]',
    '[role="menuitem"]',
    'summary',
    '[contenteditable="true"]',
    '[contenteditable=""]',
  ].join(', ');

  const MAX_ITEM_NAME_CHARS = 80;

  function isVisible(el) {
    if (!(el instanceof HTMLElement)) return false;
    const style = getComputedStyle(el);
    if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }

  function isInViewport(el) {
    const rect = el.getBoundingClientRect();
    return rect.bottom >= 0 && rect.top <= window.innerHeight && rect.right >= 0 && rect.left <= window.innerWidth;
  }

  function clean(text) {
    return String(text).replace(/\s+/g, ' ').trim();
  }

  function elementText(el) {
    if (el instanceof HTMLElement && typeof el.innerText === 'string') return el.innerText;
    return el.textContent ?? '';
  }

  function truncate(text, max) {
    if (text.length <= max) return { text, truncated: 0 };
    return { text: `${text.slice(0, max)}…`, truncated: text.length - max };
  }

  function cssEscape(value) {
    if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') return CSS.escape(value);
    return value.replace(/[^a-zA-Z0-9_-]/g, (ch) => `\\${ch}`);
  }

  function accessibleName(el) {
    const shortName = (text) => truncate(clean(redactFields(text)), MAX_ITEM_NAME_CHARS).text;
    const ariaLabel = el.getAttribute('aria-label');
    if (ariaLabel !== null && ariaLabel.trim() !== '') return shortName(ariaLabel);

    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy !== null) {
      const ref = document.getElementById(labelledBy.split(/\s+/)[0] ?? '');
      if (ref?.textContent?.trim()) return shortName(ref.textContent);
    }

    const labelable = el instanceof HTMLInputElement || el instanceof HTMLSelectElement || el instanceof HTMLTextAreaElement;
    if (labelable) {
      if (el.id !== '') {
        const label = el.ownerDocument.querySelector(`label[for="${cssEscape(el.id)}"]`);
        if (label?.textContent?.trim()) return shortName(label.textContent);
      }
      const wrapping = el.closest('label');
      if (wrapping?.textContent?.trim()) return shortName(wrapping.textContent);
    }

    const ownText = el instanceof HTMLInputElement ? '' : el.textContent;
    if (ownText?.trim()) return shortName(ownText);

    if (el instanceof HTMLInputElement) {
      const buttonLike = el.type === 'submit' || el.type === 'button' || el.type === 'reset';
      if (buttonLike && el.value !== '') return shortName(el.value);
      if (el.placeholder !== '') return shortName(el.placeholder);
      if (el.alt !== '') return shortName(el.alt);
      return shortName(el.type);
    }
    return el.tagName.toLowerCase();
  }

  function collectInteractive(root) {
    const seen = new Set();
    const result = [];
    for (const el of root.querySelectorAll(INTERACTIVE_SELECTOR)) {
      if (seen.has(el)) continue;
      seen.add(el);
      if (isVisible(el)) result.push(el);
    }
    return result;
  }

  function mainText(doc) {
    const main = doc.querySelector('main, [role="main"]');
    if (main !== null) return clean(elementText(main));
    const articles = doc.querySelectorAll('article');
    if (articles.length === 1) return clean(elementText(articles[0]));

    let best = null;
    let bestScore = 0;
    for (const candidate of doc.querySelectorAll('section, div, [role="main"]')) {
      const paragraphs = candidate.querySelectorAll('p').length;
      if (paragraphs < 2) continue;
      const text = elementText(candidate);
      const score = text.length * Math.min(paragraphs, 5);
      if (score > bestScore) {
        bestScore = score;
        best = candidate;
      }
    }
    if (best !== null) return clean(elementText(best));
    return clean(elementText(doc.body));
  }

  function pageText(root) {
    const source = root ?? document.body;
    if (source === null || source === undefined) return '';
    return clean(elementText(source));
  }

  // ── ids：稳定编号 ─────────────────────────────────────────

  const ID_ATTRIBUTE = 'data-vivian-el';

  class ElementIds {
    constructor() {
      this.idByElement = new WeakMap();
      this.elementById = new Map();
      this.nextId = 1;
    }
    assign(elements) {
      const seen = new Set(elements);
      let removed = 0;
      for (const [id, el] of this.elementById) {
        if (!seen.has(el)) {
          this.elementById.delete(id);
          this.idByElement.delete(el);
          removed += 1;
        }
      }
      let added = 0;
      for (const el of elements) {
        if (!this.idByElement.has(el)) {
          const id = this.nextId;
          this.nextId += 1;
          this.idByElement.set(el, id);
          this.elementById.set(id, el);
          try { el.setAttribute(ID_ATTRIBUTE, String(id)); } catch (_) { /* 只读节点忽略 */ }
          added += 1;
        }
      }
      return { added, removed };
    }
    indexOf(el) { return this.idByElement.get(el); }
    elementByIndex(index) { return this.elementById.get(index); }
  }

  // ── privacy：敏感字段绝不回传 ─────────────────────────────

  const SENSITIVE_PATTERNS = [/password/i, /passwd/i, /credit/i, /card/i, /cvv/i, /cvc/i, /secret/i, /pwd/i, /token/i, /api.?key/i, /otp/i, /密码|验证码/];

  function isSensitiveField(el) {
    if (el instanceof HTMLInputElement) {
      if (el.type === 'password') return true;
      const autocomplete = String(el.autocomplete);
      if (autocomplete === 'credit-card' || autocomplete.startsWith('cc-')) return true;
    }
    const name = el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement ? el.name : '';
    const haystack = [el.id, name, el.getAttribute('aria-label')].filter(Boolean).join(' ');
    return SENSITIVE_PATTERNS.some((pattern) => pattern.test(haystack));
  }

  function maskValue(value) {
    return value.length === 0 ? '' : '••••';
  }

  function redactFields(text) {
    let result = String(text);
    for (const field of document.querySelectorAll('input, textarea, select')) {
      if (!isSensitiveField(field)) continue;
      const value = field.value;
      if (typeof value !== 'string') continue;
      for (const secret of new Set([value, clean(value)])) {
        if (secret.length > 0) result = result.split(secret).join('••••');
      }
    }
    return result;
  }

  // ── snapshot ──────────────────────────────────────────────

  const SNAPSHOT_BUDGET = { maxItems: 60, maxForms: 300, maxChars: 32000 };

  function roleOf(el) {
    const role = el.getAttribute('role');
    if (role) return role;
    if (el instanceof HTMLAnchorElement) return 'link';
    if (el instanceof HTMLButtonElement) return 'button';
    if (el instanceof HTMLInputElement) {
      if (el.type === 'checkbox') return 'checkbox';
      if (el.type === 'radio') return 'radio';
      return 'input';
    }
    if (el instanceof HTMLSelectElement) return 'select';
    if (el instanceof HTMLTextAreaElement) return 'textarea';
    if (el instanceof HTMLElement && el.isContentEditable) return 'contenteditable';
    return el.tagName.toLowerCase();
  }

  function hrefHeadline(href) {
    try {
      const url = new URL(href, document.baseURI);
      return url.origin === location.origin ? `${url.pathname}${url.search}` : `${url.host}${url.pathname}`;
    } catch (_) {
      return href;
    }
  }

  function buildSnapshot(ids, options, last) {
    const elements = collectInteractive(document);
    const { added, removed } = ids.assign(elements);
    const reindexed = last !== null && added + removed > elements.length * 0.5;

    const elementViews = elements.map((element) => ({ element, inViewport: isInViewport(element) }));
    const ordered = [...elementViews].sort((a, b) => Number(b.inViewport) - Number(a.inViewport));
    const names = new Map();
    const nameOf = (element) => {
      let name = names.get(element);
      if (name === undefined) {
        name = accessibleName(element);
        names.set(element, name);
      }
      return name;
    };

    const items = [];
    for (const { element: el, inViewport } of ordered.slice(0, options.budget.maxItems)) {
      const index = ids.indexOf(el);
      if (index === undefined) continue;
      const item = { index, role: roleOf(el), name: nameOf(el), inViewport };
      if (isDisabled(el)) item.disabled = true;
      if (el instanceof HTMLInputElement) {
        if (el.disabled) item.disabled = true;
        if (el.type === 'checkbox' || el.type === 'radio') item.checked = el.checked;
      }
      const ariaChecked = el.getAttribute('aria-checked');
      if (ariaChecked === 'true' || ariaChecked === 'false') item.checked = ariaChecked === 'true';
      if (el instanceof HTMLOptionElement && el.selected) item.selected = true;
      if (el instanceof HTMLAnchorElement && el.href !== '') item.href = hrefHeadline(el.href);
      items.push(item);
    }

    const formElements = elements.filter((el) => el instanceof HTMLInputElement
      || el instanceof HTMLSelectElement
      || el instanceof HTMLTextAreaElement);
    const forms = [];
    for (const el of formElements.slice(0, options.budget.maxForms)) {
      const index = ids.indexOf(el);
      if (index === undefined) continue;
      const masked = isSensitiveField(el);
      const checkable = el instanceof HTMLInputElement && (el.type === 'checkbox' || el.type === 'radio');
      const value = checkable
        ? ''
        : el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement
          ? el.value
          : el instanceof HTMLSelectElement
            ? [...el.selectedOptions].map((o) => o.textContent ?? '').join(', ')
            : '';
      const field = {
        index,
        label: nameOf(el),
        kind: el instanceof HTMLInputElement ? el.type : el.tagName.toLowerCase(),
        value: masked ? maskValue(value) : value.slice(0, 120),
        masked,
      };
      if (checkable) field.checked = el.checked;
      if (el instanceof HTMLInputElement && el.required) field.required = true;
      field.disabled = isDisabled(el);
      field.readonly = Boolean(el.readOnly || el.closest('[aria-readonly="true"]'));
      if (el instanceof HTMLSelectElement && !masked) {
        field.options = [...el.options].slice(0, 40).map(option => ({
          value: option.value.slice(0, 128), label: clean(option.textContent ?? '').slice(0, 80),
          disabled: option.disabled || (option.parentElement?.tagName === 'OPTGROUP' && option.parentElement.disabled),
          selected: option.selected,
        }));
        field.optionsOmitted = Math.max(0, el.options.length - 40);
      }
      forms.push(field);
    }

    const regionEl = options.region !== undefined && options.region !== ''
      ? document.querySelector(options.region)
      : null;
    const mainSource = redactFields(regionEl !== null ? pageText(regionEl) : mainText(document));
    const mainBudget = Math.floor(options.budget.maxChars * 0.5);
    const main = truncate(mainSource, mainBudget);

    const lastItems = last === null ? new Map() : new Map(last.items.map((item) => [item.index, item]));
    const lastForms = last === null ? new Map() : new Map(last.forms.map((form) => [form.index, form]));

    const changed = new Set();
    const removedIds = [];
    if (options.delta === true && last !== null) {
      if (last.main !== main.text || last.url !== location.href || last.title !== document.title) changed.add(-1);
      for (const item of items) {
        const before = lastItems.get(item.index);
        if (before === undefined || !sameItem(before, item)) changed.add(item.index);
      }
      const currentItemIds = new Set(items.map((item) => item.index));
      for (const index of lastItems.keys()) {
        if (!currentItemIds.has(index)) removedIds.push(index);
      }
      for (const form of forms) {
        const before = lastForms.get(form.index);
        if (before === undefined || !sameForm(before, form)) changed.add(form.index);
      }
    }

    return {
      version: (last?.version ?? 0) + 1,
      url: location.href,
      title: document.title,
      ready: document.readyState === 'complete' ? 'complete' : 'loading',
      main: main.text,
      items,
      forms,
      changed: options.delta === true ? [...changed] : [],
      removed: options.delta === true ? removedIds : [],
      reindexed,
      truncated: {
        mainChars: main.truncated,
        itemsDropped: Math.max(0, elements.length - options.budget.maxItems),
        formsDropped: Math.max(0, formElements.length - options.budget.maxForms),
      },
      budgetChars: options.budget.maxChars,
    };
  }

  function sameItem(a, b) {
    return a.role === b.role && a.name === b.name && a.href === b.href
      && a.disabled === b.disabled && a.checked === b.checked && a.inViewport === b.inViewport;
  }

  function sameForm(a, b) {
    return a.label === b.label && a.kind === b.kind && a.value === b.value && a.masked === b.masked
      && a.checked === b.checked && a.required === b.required && a.disabled === b.disabled && a.readonly === b.readonly
      && JSON.stringify(a.options) === JSON.stringify(b.options) && a.optionsOmitted === b.optionsOmitted;
  }

  function capRendered(text, budgetChars) {
    if (text.length <= budgetChars) return text;
    return `${text.slice(0, budgetChars)}…(truncated to the snapshot character budget)`;
  }

  function renderItem(item) {
    const state = [
      item.disabled === true ? 'disabled' : undefined,
      item.checked === undefined ? undefined : item.checked ? 'checked' : 'unchecked',
      item.inViewport ? undefined : 'outside viewport',
    ].filter((v) => v !== undefined).join('/');
    const stateText = state === '' ? '' : ` [${state}]`;
    const hrefText = item.href !== undefined ? ` → ${item.href}` : '';
    return `  [${item.index}] ${item.role} "${item.name}"${stateText}${hrefText}`;
  }

  function renderForm(form, includeIdentity) {
    const identity = includeIdentity ? `${form.label} (${form.kind}) ` : '';
    const text = form.checked === undefined
      ? `value="${form.masked ? '••••' : form.value}"`
      : `checked=${String(form.checked)}`;
    const options = form.options ? ` options=${JSON.stringify(form.options)}${form.optionsOmitted ? ` (${form.optionsOmitted} omitted)` : ''}` : '';
    return `  [${form.index}] ${identity}${text}${form.required === true ? ' required' : ''}${form.disabled ? ' disabled' : ''}${form.readonly ? ' readonly' : ''}${options}`;
  }

  function appendTruncationNotes(lines, view) {
    const notes = [];
    if (view.truncated.mainChars > 0) notes.push(`Main content truncated by ${view.truncated.mainChars} characters`);
    if (view.truncated.itemsDropped > 0) notes.push(`${view.truncated.itemsDropped} additional elements omitted`);
    if (view.truncated.formsDropped > 0) notes.push(`${view.truncated.formsDropped} additional form fields omitted`);
    if (notes.length > 0) lines.push(`\n(${notes.join('; ')}. Use browser_get_text or specify region for more content.)`);
  }

  function renderSnapshot(view, delta, maxChars = view.budgetChars) {
    const lines = [];
    if (delta) {
      lines.push(`Page change v${view.version} (${view.url})`);
      const elementChanges = view.changed.filter((id) => id !== -1);
      const changedIds = new Set(elementChanges);
      const changedItems = view.items.filter((item) => changedIds.has(item.index));
      const changedForms = view.forms.filter((form) => changedIds.has(form.index));

      lines.push(`Status: ${view.ready}${view.reindexed ? ' (element indices were reassigned; use the indices in this snapshot)' : ''}`);
      if (view.changed.includes(-1)) {
        lines.push(`Title: ${view.title || '(untitled)'}`);
        if (view.main.length > 0) {
          lines.push('');
          lines.push('Changed main content:');
          lines.push(view.main);
        }
      }
      if (changedItems.length > 0) {
        lines.push('');
        lines.push('Changed interactive elements:');
        for (const item of changedItems) lines.push(renderItem(item));
      }
      if (changedForms.length > 0) {
        lines.push('');
        lines.push('Changed form fields:');
        const renderedItems = new Set(changedItems.map((item) => item.index));
        for (const form of changedForms) lines.push(renderForm(form, !renderedItems.has(form.index)));
      }
      if (view.removed.length > 0) lines.push(`Removed elements: ${view.removed.join(', ')}`);
      if (view.changed.length === 0 && view.removed.length === 0) lines.push('(No visible changes.)');
      appendTruncationNotes(lines, view);
      return capRendered(lines.join('\n'), maxChars);
    }

    lines.push(`Title: ${view.title || '(untitled)'}`);
    lines.push(`URL: ${view.url}`);
    lines.push(`Status: ${view.ready}${view.reindexed ? ' (element indices were reassigned; use the indices in this snapshot)' : ''}`);
    if (view.main.length > 0) {
      lines.push('');
      lines.push('Main content:');
      lines.push(view.main);
    }
    if (view.items.length > 0) {
      lines.push('');
      lines.push('Interactive elements:');
      for (const item of view.items) lines.push(renderItem(item));
    }
    if (view.forms.length > 0) {
      lines.push('');
      lines.push('Form fields:');
      const renderedItems = new Set(view.items.map((item) => item.index));
      for (const form of view.forms) lines.push(renderForm(form, !renderedItems.has(form.index)));
    }
    appendTruncationNotes(lines, view);
    return capRendered(lines.join('\n'), maxChars);
  }

  // ── actions ───────────────────────────────────────────────

  const ACTION_SETTLE = { minimumMs: 100, quietMs: 50, maxAfterReadyMs: 250, timeoutMs: 5000 };
  const SCROLL_SETTLE = { minimumMs: 50, quietMs: 50, maxAfterReadyMs: 150, timeoutMs: 5000 };
  const EXPLICIT_WAIT_SETTLE = { minimumMs: 100, quietMs: 100, maxAfterReadyMs: 1000, timeoutMs: 5000 };

  let lastSnapshot = null;

  function resetDeltaState() { lastSnapshot = null; }

  function waitForPageSettled(policy = ACTION_SETTLE) {
    const startedAt = performance.now();
    let readyAt = document.readyState === 'complete' ? startedAt : undefined;
    let lastMutationAt = startedAt;
    let timer;
    let finished = false;
    let observer;

    return new Promise((resolve) => {
      const finish = (settled) => {
        if (finished) return;
        finished = true;
        if (timer !== undefined) clearTimeout(timer);
        observer?.disconnect();
        document.removeEventListener('readystatechange', schedule);
        window.removeEventListener('load', schedule);
        resolve(settled);
      };
      const check = () => {
        timer = undefined;
        const now = performance.now();
        if (readyAt === undefined && document.readyState === 'complete') {
          readyAt = now;
          lastMutationAt = now;
        }
        if (readyAt !== undefined) {
          const afterReady = now - readyAt;
          const quietFor = now - lastMutationAt;
          if ((afterReady >= policy.minimumMs && quietFor >= policy.quietMs) || afterReady >= policy.maxAfterReadyMs) {
            finish(true);
            return;
          }
          const untilMinimum = Math.max(0, policy.minimumMs - afterReady);
          const untilQuiet = Math.max(0, policy.quietMs - quietFor);
          timer = setTimeout(check, Math.max(1, Math.min(policy.maxAfterReadyMs - afterReady, Math.max(untilMinimum, untilQuiet))));
          return;
        }
        const elapsed = now - startedAt;
        if (elapsed >= policy.timeoutMs) {
          finish(false);
          return;
        }
        timer = setTimeout(check, Math.max(1, Math.min(100, policy.timeoutMs - elapsed)));
      };
      function schedule() {
        if (finished) return;
        if (timer !== undefined) clearTimeout(timer);
        timer = setTimeout(check, 0);
      }

      if (document.documentElement !== null) {
        observer = new MutationObserver(() => {
          lastMutationAt = performance.now();
          schedule();
        });
        observer.observe(document.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
      }
      document.addEventListener('readystatechange', schedule);
      window.addEventListener('load', schedule);
      schedule();
    });
  }

  function sleep(ms) { return new Promise((resolve) => { setTimeout(resolve, ms); }); }

  class ActionError extends Error {
    constructor(code, message) {
      super(message);
      this.code = code;
      this.name = 'ActionError';
    }
  }

  const ids = new ElementIds();

  function isDisabled(el) {
    return el.matches(':disabled') || el.closest('[aria-disabled="true"], [inert]') !== null;
  }

  function elementOrThrow(index) {
    const el = ids.elementByIndex(index);
    if (el === undefined || !el.isConnected || !isVisible(el)) {
      throw new ActionError('action-failed', `Element [${index}] does not exist; the page may have changed. Call browser_snapshot again to get current indices.`);
    }
    return el;
  }

  function numberArg(args, name) {
    const value = args[name];
    if (typeof value !== 'number' || !Number.isInteger(value) || value < 0) {
      throw new ActionError('bad-args', `${name} must be a non-negative integer; received ${String(value)}.`);
    }
    return value;
  }

  function resolveTargetAction(args) {
    const locator = args.locator;
    if (!locator || typeof locator !== 'object') throw new ActionError('bad-args', 'A locator is required.');
    let root = document;
    if (locator.scope) {
      const scopes = [...document.querySelectorAll(locator.scope)];
      if (scopes.length !== 1) throw new ActionError('ambiguous-target', 'Scope must match exactly one region.');
      root = scopes[0];
    }
    let matches = [...root.querySelectorAll(locator.selector || INTERACTIVE_SELECTOR)];
    if (locator.role) matches = matches.filter(el => roleOf(el) === locator.role);
    if (locator.name !== undefined) matches = matches.filter(el => accessibleName(el) === locator.name);
    if (matches.length !== 1) throw new ActionError('ambiguous-target', `Locator matched ${matches.length} elements; refine selector, role, name or scope. No input sent.`);
    ids.assign(collectInteractive(document));
    const index = ids.indexOf(matches[0]);
    if (index === undefined) throw new ActionError('bad-target', 'Locator must resolve to an interactive snapshot target.');
    return { index };
  }

  function snapshotAction(args) {
    const delta = args.delta === true;
    const region = typeof args.region === 'string' && args.region !== '' ? args.region : undefined;
    const view = buildSnapshot(ids, { delta, region, budget: SNAPSHOT_BUDGET }, lastSnapshot);
    lastSnapshot = view;
    return { text: redactFields(renderSnapshot(view, delta)) };
  }

  function withPageDelta(text) {
    if (lastSnapshot === null) return { text };
    const view = buildSnapshot(ids, { delta: true, budget: SNAPSHOT_BUDGET }, lastSnapshot);
    lastSnapshot = view;
    return {
      text,
      pageContent: redactFields(renderSnapshot(view, true, Math.min(SNAPSHOT_BUDGET.maxChars, 4000))),
    };
  }

  function selectOptionAction(args) {
    const el = elementOrThrow(numberArg(args, 'index'));
    if (el.tagName !== 'SELECT' || isDisabled(el)) throw new ActionError('bad-target', 'Target must be an enabled native select control.');
    if (typeof args.value !== 'string') throw new ActionError('bad-args', 'value must be a string.');
    const choices = Array.from(el.options).filter(option => option.value === args.value && !option.disabled && !(option.parentElement?.tagName === 'OPTGROUP' && option.parentElement.disabled));
    if (choices.length !== 1) throw new ActionError('bad-target', 'Expected exactly one enabled option with that value.');
    el.value = args.value;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return withPageDelta('Selected the requested option; inspect the resulting state.');
  }

  // Only preparation runs in this isolated script. Do not synthesize click/key/input
  // events or assign value/textContent: that bypasses browser editing and isTrusted.
  async function inputTargetAction(args, kind) {
    const index = numberArg(args, 'index');
    const el = ids.elementByIndex(index);
    const timeout = args.timeout_ms ?? 1500;
    if (!Number.isInteger(timeout) || timeout < 0 || timeout > 5000) throw new ActionError('bad-args', 'timeout_ms must be an integer between 0 and 5000.');
    const deadline = performance.now() + timeout;
    let previous = '';
    let reason = 'Target is not actionable';
    do {
      if (!el?.isConnected) throw new ActionError('stale-target', 'Target was detached; read a fresh snapshot.');
      if (isVisible(el)) {
        el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
        const rect = el.getBoundingClientRect();
        // Use the visible intersection, not a clamped off-screen center.
        const left = Math.max(0, rect.left), right = Math.min(innerWidth, rect.right);
        const top = Math.max(0, rect.top), bottom = Math.min(innerHeight, rect.bottom);
        const x = (left + right) / 2, y = (top + bottom) / 2;
        const hit = document.elementFromPoint(x, y);
        const position = [rect.left, rect.top, rect.width, rect.height].join(',');
        const enabled = kind === 'hover' || !isDisabled(el);
        const uncovered = left < right && top < bottom && hit && (hit === el || el.contains(hit));
        if (enabled && uncovered && previous === position) {
          if (kind === 'type') {
            const editable = el instanceof HTMLTextAreaElement || el.isContentEditable
              || (el instanceof HTMLInputElement && ['text', 'search', 'email', 'url', 'tel', 'password'].includes(el.type));
            if (!editable || el.readOnly || el.closest('[aria-readonly="true"]')) throw new ActionError('not-editable', 'Target must be an enabled, writable text field or contenteditable.');
            el.focus({ preventScroll: true });
            if (document.activeElement !== el && !el.contains(document.activeElement)) throw new ActionError('focus-failed', 'Editable target did not receive focus.');
          }
          return { x, y };
        }
        reason = !enabled ? 'Target is disabled or inert' : !uncovered ? 'Target is covered or outside the viewport' : 'Target is moving';
        previous = position;
      } else { previous = ''; reason = 'Target is hidden'; }
      if (performance.now() >= deadline) break;
      await sleep(Math.min(50, Math.max(1, deadline - performance.now())));
    } while (true);
    throw new ActionError('not-actionable', `${reason}; no input sent. Observe again.`);
  }

  async function scrollAction(args) {
    const direction = typeof args.direction === 'string' ? args.direction : '';
    const amount = typeof args.amount === 'number' ? args.amount : Math.floor(window.innerHeight * 0.8);
    switch (direction) {
      case 'top': window.scrollTo({ top: 0, behavior: 'instant' }); break;
      case 'bottom': window.scrollTo({ top: document.documentElement.scrollHeight, behavior: 'instant' }); break;
      case 'up': window.scrollBy({ top: -amount, behavior: 'instant' }); break;
      case 'down': window.scrollBy({ top: amount, behavior: 'instant' }); break;
      default: throw new ActionError('bad-args', `direction must be up, down, top, or bottom; received "${direction}".`);
    }
    await waitForPageSettled(SCROLL_SETTLE);
    return withPageDelta(`Scrolled ${direction}.`);
  }

  async function navigateAction(args) {
    const url = typeof args.url === 'string' && args.url !== '' ? args.url : '';
    if (url === '') throw new ActionError('bad-args', 'url must not be empty.');
    let parsed;
    try { parsed = new URL(url); } catch (_) { throw new ActionError('bad-args', `url is not valid: ${url}`); }
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      throw new ActionError('bad-args', `Only http and https URLs are supported; received ${parsed.protocol}.`);
    }
    resetDeltaState();
    setTimeout(() => { location.href = parsed.href; }, 0);
    return {
      text: `Navigating to ${parsed.href}. Call browser_snapshot again after the page loads.`,
      navigationPending: true,
    };
  }

  async function historyAction(delta) {
    resetDeltaState();
    setTimeout(() => { if (delta === -1) history.back(); else history.forward(); }, 0);
    return {
      text: 'Navigating through browser history. Call browser_snapshot again after the page loads.',
      navigationPending: true,
    };
  }

  function reloadAction() {
    resetDeltaState();
    setTimeout(() => { location.reload(); }, 0);
    return {
      text: 'The page is reloading. Call browser_snapshot again after it loads.',
      navigationPending: true,
    };
  }

  async function getTextAction(args) {
    const selector = typeof args.selector === 'string' && args.selector !== '' ? args.selector : undefined;
    const source = selector !== undefined ? document.querySelector(selector) : null;
    const text = source !== null ? pageText(source) : selector !== undefined ? `No element matched selector: ${selector}` : pageText();
    const offset = Number.isInteger(args.offset) && args.offset >= 0 ? args.offset : 0;
    const truncated = truncate(redactFields(text).slice(offset), 16000);
    return { text: truncated.text + (truncated.truncated > 0 ? `\n(Truncated ${truncated.truncated} characters.)` : '') };
  }

  async function waitAction(args) {
    if (args.selector !== undefined || args.text !== undefined) {
      if (args.selector !== undefined && (typeof args.selector !== 'string' || !args.selector.trim())) throw new ActionError('bad-args', 'selector must be non-empty.');
      if (args.text !== undefined && (typeof args.text !== 'string' || !args.text)) throw new ActionError('bad-args', 'text must be non-empty.');
      const state = args.state ?? 'visible';
      if (!['attached', 'detached', 'visible', 'hidden'].includes(state)) throw new ActionError('bad-args', 'Unsupported wait state.');
      if (args.text !== undefined && ['hidden', 'detached'].includes(state)) throw new ActionError('bad-args', 'Text conditions require visible or attached state.');
      const timeout = args.timeout_ms ?? 5000;
      if (!Number.isInteger(timeout) || timeout < 0 || timeout > 10000) throw new ActionError('bad-args', 'timeout_ms must be between 0 and 10000.');
      const deadline = performance.now() + timeout;
      do {
        const matches = args.selector === undefined ? [document.body] : [...document.querySelectorAll(args.selector)];
        if (matches.length > 1) throw new ActionError('ambiguous-target', 'Wait selector matched more than one element.');
        const el = matches[0];
        const attached = Boolean(el?.isConnected), visible = attached && isVisible(el);
        const matched = state === 'attached' ? attached : state === 'detached' ? !attached : state === 'visible' ? visible : !visible;
        if (matched && (args.text === undefined || redactFields(pageText(el)).includes(args.text))) return withPageDelta('Requested page condition is satisfied.');
        if (performance.now() >= deadline) break;
        await sleep(Math.min(50, Math.max(1, deadline - performance.now())));
      } while (true);
      throw new ActionError('condition-timeout', 'Page condition was not satisfied before timeout.');
    }
    const ms = typeof args.ms === 'number' && args.ms > 0 ? Math.min(args.ms, 10000) : 0;
    await waitForPageSettled(EXPLICIT_WAIT_SETTLE);
    if (ms > 0) await sleep(ms);
    return withPageDelta(`The page is stable${ms > 0 ? ` after an additional ${ms}ms wait` : ''}.`);
  }

  async function runAction(action, args) {
    switch (action) {
      case 'browser_snapshot': return snapshotAction(args);
      case 'browser_resolve_target': return resolveTargetAction(args);
      case 'browser_select_option': return selectOptionAction(args);
      case 'browser_hover_target': return inputTargetAction(args, 'hover');
      case 'browser_click_target': return inputTargetAction(args, 'click');
      case 'browser_type_target': return inputTargetAction(args, 'type');
      case 'browser_scroll': return scrollAction(args);
      case 'browser_navigate': return navigateAction(args);
      case 'browser_back': return historyAction(-1);
      case 'browser_forward': return historyAction(1);
      case 'browser_reload': return reloadAction();
      case 'browser_get_text': return getTextAction(args);
      case 'browser_wait': return waitAction(args);
      case 'browser_eval_js': return evalJsAction(args);
      case 'browser_extract': return extractAction(args);
      case 'browser_platform_detail': {
        await waitForPageSettled(EXPLICIT_WAIT_SETTLE);
        // 评论区多为懒加载：滚到底再等一会儿，只取当前可见的少量评论
        try {
          window.scrollTo(0, document.body.scrollHeight);
          await sleep(600);
        } catch (_) { /* 页面不允许滚动时按可见内容提取 */ }
        return { text: JSON.stringify(globalThis.vivianExtractDetail()) };
      }
      case 'browser_bilibili_history': {
        if (location.hostname !== 'api.bilibili.com' || location.pathname !== '/x/web-interface/history/cursor') throw new ActionError('wrong-platform', 'Not a Bilibili history endpoint');
        const body = JSON.parse(document.body.textContent);
        if (body.code !== 0) throw new ActionError('login-required', 'B站会话无效或需要验证，请在内嵌浏览器确认。');
        const limit = Math.max(1, Math.min(100, Number(args.limit) || 50));
        return { text: JSON.stringify((body.data?.list || []).slice(0, limit).map(item => ({ title: String(item.title || '').slice(0, 200) }))) };
      }
      default: throw new ActionError('bad-args', `Unknown action: ${action}`);
    }
  }

  // Legacy calls fail explicitly; platform extraction uses packaged functions.
  function evalJsAction() {
    throw new ActionError('unsupported-action', 'Use browser_snapshot, browser_get_text or browser_extract; arbitrary code is not a browser tool.');
  }

  function extractAction(args) {
    try {
      if (args.extractor === 'images') {
        const images = [];
        const seen = new Set();
        for (const img of document.querySelectorAll('img')) {
          if (!isVisible(img)) continue;
          const raw = img.currentSrc || img.getAttribute('src');
          if (!raw) continue;
          let url;
          try { url = new URL(raw, location.href); } catch (_) { continue; }
          if (!['http:', 'https:'].includes(url.protocol) || seen.has(url.href)) continue;
          // Ignore loaded tracking pixels; keep lazy images whose dimensions are not known yet.
          if (img.naturalWidth > 0 && img.naturalHeight > 0
              && (img.naturalWidth < 32 || img.naturalHeight < 32)) continue;
          seen.add(url.href);
          images.push({ url: url.href, alt: clean(img.getAttribute('alt') || '').slice(0, 200),
            source_url: location.href, width: img.naturalWidth || 0, height: img.naturalHeight || 0 });
          if (images.length >= 30) break;
        }
        return { text: JSON.stringify(images) };
      }
      return { text: JSON.stringify(globalThis.vivianExtractPlatform(args.extractor)) };
    } catch (error) {
      throw new ActionError('extraction-failed', error.message);
    }
  }

  return runAction;
};
