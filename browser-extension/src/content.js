/**
 * Vivian 浏览器桥 content script。
 *
 * 在受控标签页内运行（每份文档一个实例），把页面渲染成结构化文本快照，并按
 * 稳定编号在真实页面里执行点击 / 输入 / 按键 / 滚动 / 导航等动作，保留登录态。
 * 整条管线纯文本，无截图——页面文字的份额由快照/动作结果承载。
 *
 * 能力面：extract（元素提取）/ ids（稳定编号）/ snapshot（结构化快照）/
 * actions（页面动作）/ privacy（敏感字段掩码）。
 */

(() => {
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
    const ariaLabel = el.getAttribute('aria-label');
    if (ariaLabel !== null && ariaLabel.trim() !== '') return truncate(clean(ariaLabel), MAX_ITEM_NAME_CHARS).text;

    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy !== null) {
      const ref = document.getElementById(labelledBy.split(/\s+/)[0] ?? '');
      if (ref?.textContent?.trim()) return truncate(clean(ref.textContent), MAX_ITEM_NAME_CHARS).text;
    }

    const labelable = el instanceof HTMLInputElement || el instanceof HTMLSelectElement || el instanceof HTMLTextAreaElement;
    if (labelable) {
      if (el.id !== '') {
        const label = el.ownerDocument.querySelector(`label[for="${cssEscape(el.id)}"]`);
        if (label?.textContent?.trim()) return truncate(clean(label.textContent), MAX_ITEM_NAME_CHARS).text;
      }
      const wrapping = el.closest('label');
      if (wrapping?.textContent?.trim()) return truncate(clean(wrapping.textContent), MAX_ITEM_NAME_CHARS).text;
    }

    const ownText = el instanceof HTMLInputElement ? '' : el.textContent;
    if (ownText?.trim()) return truncate(clean(ownText), MAX_ITEM_NAME_CHARS).text;

    if (el instanceof HTMLInputElement) {
      const buttonLike = el.type === 'submit' || el.type === 'button' || el.type === 'reset';
      if (buttonLike && el.value !== '') return truncate(clean(el.value), MAX_ITEM_NAME_CHARS).text;
      if (el.placeholder !== '') return truncate(clean(el.placeholder), MAX_ITEM_NAME_CHARS).text;
      if (el.alt !== '') return truncate(clean(el.alt), MAX_ITEM_NAME_CHARS).text;
      return truncate(clean(el.type), MAX_ITEM_NAME_CHARS).text;
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

  const SENSITIVE_PATTERNS = [/password/i, /passwd/i, /credit/i, /card/i, /cvv/i, /cvc/i, /secret/i, /pwd/i];

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
      if (el instanceof HTMLButtonElement && el.disabled) item.disabled = true;
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
      forms.push(field);
    }

    const regionEl = options.region !== undefined && options.region !== ''
      ? document.querySelector(options.region)
      : null;
    const mainSource = regionEl !== null ? pageText(regionEl) : mainText(document);
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
      && a.checked === b.checked && a.required === b.required;
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
    return `  [${form.index}] ${identity}${text}${form.required === true ? ' required' : ''}`;
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

  const TYPE_SETTLE = { minimumMs: 32, quietMs: 32, maxAfterReadyMs: 100, timeoutMs: 5000 };
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

  function setNativeValue(input, value) {
    const prototype = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
    if (setter === undefined) input.value = value;
    else setter.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }

  const ids = new ElementIds();

  function elementOrThrow(index) {
    const el = ids.elementByIndex(index);
    if (el === undefined) {
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

  function snapshotAction(args) {
    const delta = args.delta === true;
    const region = typeof args.region === 'string' && args.region !== '' ? args.region : undefined;
    const view = buildSnapshot(ids, { delta, region, budget: SNAPSHOT_BUDGET }, lastSnapshot);
    lastSnapshot = view;
    return { text: renderSnapshot(view, delta) };
  }

  function withPageDelta(text) {
    if (lastSnapshot === null) return { text };
    const view = buildSnapshot(ids, { delta: true, budget: SNAPSHOT_BUDGET }, lastSnapshot);
    lastSnapshot = view;
    return {
      text,
      pageContent: renderSnapshot(view, true, Math.min(SNAPSHOT_BUDGET.maxChars, 4000)),
    };
  }

  async function clickAction(args) {
    const index = numberArg(args, 'index');
    const el = elementOrThrow(index);
    el.scrollIntoView({ block: 'center', behavior: 'instant' });
    if (el instanceof HTMLAnchorElement) {
      const target = el.target.trim().toLowerCase();
      const sameFrameTarget = target === '' || target === '_self';
      let href;
      try { href = new URL(el.href); } catch (_) { /* 异常链接交给原生点击 */ }
      const controlledNavigation = sameFrameTarget
        && !el.hasAttribute('download')
        && (href?.protocol === 'http:' || href?.protocol === 'https:');
      if (controlledNavigation && href !== undefined) {
        const hasReferrerPolicy = typeof el.referrerPolicy === 'string' && el.referrerPolicy !== '';
        const requiresNativeActivation = el.relList.contains('noreferrer')
          || hasReferrerPolicy
          || el.hasAttribute('ping')
          || el.hasAttribute('attributionsrc');
        if (requiresNativeActivation) {
          setTimeout(() => { el.click(); }, 0);
          return { text: `Clicked link [${index}] using native browser activation. Call browser_snapshot to read the resulting state.` };
        }
        const shouldNavigate = el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, composed: true }));
        if (!shouldNavigate) {
          await waitForPageSettled(ACTION_SETTLE);
          return withPageDelta(`Clicked link [${index}].`);
        }
        const sameDocument = href.origin === location.origin
          && href.pathname === location.pathname
          && href.search === location.search;
        if (sameDocument) {
          if (href.hash !== location.hash) location.hash = href.hash;
          await waitForPageSettled(ACTION_SETTLE);
          return withPageDelta(`Clicked link [${index}].`);
        }
        setTimeout(() => { location.href = href.href; }, 0);
        return {
          text: `Clicked link [${index}]. Call browser_snapshot again after navigation settles.`,
          navigationPending: true,
        };
      }
      setTimeout(() => { el.click(); }, 0);
      return { text: `Clicked link [${index}]. The link may open outside the controlled frame.` };
    }
    if (el instanceof HTMLButtonElement && el.disabled) {
      throw new ActionError('action-failed', `Button [${index}] is disabled.`);
    }
    el.click();
    await waitForPageSettled(ACTION_SETTLE);
    return withPageDelta(`Clicked [${index}].`);
  }

  async function typeAction(args) {
    const index = numberArg(args, 'index');
    const text = typeof args.text === 'string' ? args.text : '';
    if (text === '') throw new ActionError('bad-args', 'text must not be empty.');
    const replace = args.replace === true;
    const el = elementOrThrow(index);
    const contentEditable = el instanceof HTMLElement && el.isContentEditable;
    if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || contentEditable)) {
      throw new ActionError('action-failed', `Element [${index}] is not editable (${el.tagName.toLowerCase()}).`);
    }
    if (contentEditable) {
      if (replace) el.textContent = '';
      el.textContent = `${el.textContent ?? ''}${text}`;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    } else if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
      if (replace) setNativeValue(el, '');
      setNativeValue(el, `${el.value}${text}`);
    }
    await waitForPageSettled(TYPE_SETTLE);
    return withPageDelta(`Entered ${text.length} characters into [${index}].`);
  }

  async function pressAction(args) {
    const key = typeof args.key === 'string' && args.key !== '' ? args.key : '';
    if (key === '') throw new ActionError('bad-args', 'key must not be empty.');
    const target = document.activeElement instanceof HTMLElement ? document.activeElement : document.body;
    target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
    target.dispatchEvent(new KeyboardEvent('keyup', { key, bubbles: true, cancelable: true }));
    if (key === 'Enter' && target instanceof HTMLInputElement && target.form !== null) {
      target.form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    }
    await waitForPageSettled(ACTION_SETTLE);
    return withPageDelta(`Sent key "${key}".`);
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
    const truncated = truncate(text, 8000);
    return { text: truncated.text + (truncated.truncated > 0 ? `\n(Truncated ${truncated.truncated} characters.)` : '') };
  }

  async function waitAction(args) {
    const ms = typeof args.ms === 'number' && args.ms > 0 ? args.ms : 0;
    await waitForPageSettled(EXPLICIT_WAIT_SETTLE);
    if (ms > 0) await sleep(ms);
    return withPageDelta(`The page is stable${ms > 0 ? ` after an additional ${ms}ms wait` : ''}.`);
  }

  async function runAction(action, args) {
    switch (action) {
      case 'browser_snapshot': return snapshotAction(args);
      case 'browser_click': return clickAction(args);
      case 'browser_type': return typeAction(args);
      case 'browser_press': return pressAction(args);
      case 'browser_scroll': return scrollAction(args);
      case 'browser_navigate': return navigateAction(args);
      case 'browser_back': return historyAction(-1);
      case 'browser_forward': return historyAction(1);
      case 'browser_reload': return reloadAction();
      case 'browser_get_text': return getTextAction(args);
      case 'browser_wait': return waitAction(args);
      case 'browser_eval_js': return evalJsAction(args);
      default: throw new ActionError('bad-args', `Unknown action: ${action}`);
    }
  }

  // ── eval：在页面上下文求值 JS 表达式（高权限动作，宿主侧已要求用户确认） ──
  function evalJsAction(args) {
    const code = typeof args.code === 'string' ? args.code : '';
    if (!code) throw new ActionError('bad-args', 'code is required');
    try {
      // 表达式语义：包进 IIFE 使语句（return ...）与表达式都可用
      const fn = new Function(`"use strict"; return (async () => { ${code.includes('return') || code.trim().startsWith('(') ? code : `return (${code})`} })()`);
      const value = fn();
      if (value && typeof value.then === 'function') {
        return value
          .then((v) => ({ text: safeSerialize(v) }))
          .catch((e) => { throw new ActionError('action-failed', `eval rejected: ${e && e.message ? e.message : e}`); });
      }
      return { text: safeSerialize(value) };
    } catch (e) {
      throw new ActionError('action-failed', `eval threw: ${e && e.message ? e.message : e}`);
    }
  }

  function safeSerialize(v) {
    try {
      if (v === undefined) return 'undefined';
      return typeof v === 'string' ? v : JSON.stringify(v, null, 1);
    } catch (_) {
      return String(v);
    }
  }

  // ── 消息入口 ──────────────────────────────────────────────

  chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
    if (!msg || msg.type !== 'vivian_browser_action') return;
    runAction(msg.action, msg.args || {})
      .then((res) => sendResponse(res))
      .catch((err) => sendResponse({
        error: {
          code: err.code || 'action-failed',
          message: err.message || String(err),
        },
      }));
    return true; // 异步响应
  });
})();