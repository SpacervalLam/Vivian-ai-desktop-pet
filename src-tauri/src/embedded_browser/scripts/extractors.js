// Packaged DOM extractors for the native browser isolated world; no arbitrary code.
// DOM and URLs are untrusted page data; callers wrap results before returning them.
globalThis.vivianExtractPlatform = function (platform) {
  const plans = {
    bilibili: { host: 'bilibili.com', selector: 'a[href*="/video/BV"]', pattern: /\/video\/(BV[0-9A-Za-z]+)/ },
    xiaohongshu: { host: 'xiaohongshu.com', selector: 'a[href*="/explore/"], a[href*="/search_result/"]', pattern: /\/(?:explore|search_result)\/([0-9a-f]{8,})/ },
    douyin: { host: 'douyin.com', selector: 'a[href*="/video/"]', pattern: /\/video\/(\d+)/ },
    zhihu: { host: 'zhihu.com', selector: 'a[href*="/question/"], a[href*="/p/"]', pattern: /\/(?:question|p)\/(\d+)/ },
  };
  const plan = plans[platform];
  if (!plan) throw new Error('Unsupported extractor: ' + platform);
  const onHost = (host) => host === plan.host || host.endsWith('.' + plan.host);
  if (!onHost(location.hostname)) throw new Error('Extractor does not match the current platform');
  const items = new Map();
  for (const anchor of document.querySelectorAll(plan.selector)) {
    let url;
    try { url = new URL(anchor.getAttribute('href'), location.href); } catch (_) { continue; }
    if (!['http:', 'https:'].includes(url.protocol) || !onHost(url.hostname) || url.username || url.password) continue;
    const id = url.pathname.match(plan.pattern)?.[1];
    if (!id) continue;
    // Many feeds use separate cover/title anchors inside the same card.
    const card = anchor.closest('section, article, .note-item, .bili-video-card, .HotItem') || anchor;
    const image = card.querySelector('img');
    const title = (anchor.getAttribute('title') || anchor.textContent?.trim()
      || card.querySelector('.title, h2, h3')?.textContent || image?.alt || '').trim().slice(0, 160);
    if (!title) continue;
    const previous = items.get(id);
    // Prefer a title anchor, but preserve access tokens from the cover URL.
    if (previous) {
      if (!new URL(previous.url).search && url.search) previous.url = url.href;
      continue;
    }
    items.set(id, { id, title, desc: title, url: url.href,
      author: '', cover: image?.src || '', views: 0, likes: 0 });
    if (items.size >= 20) break;
  }
  return Array.from(items.values());
};

// Packaged detail extractor: body text + visible top comments for a single item page.
// Fixed selectors per platform — no arbitrary code. Results are untrusted page data.
// Anti-change strategy (from bb-browser): try several selectors, then fall back to
// semantic elements (article / role=main) instead of relying on one class name.
globalThis.vivianExtractDetail = function () {
  const host = location.hostname;
  const onHost = (domain) => host === domain || host.endsWith('.' + domain);
  const readText = (el) => (el ? String(el.innerText ?? el.textContent ?? '').replace(/\s+/g, ' ').trim() : '');
  const pickBody = (selectors) => {
    for (const selector of selectors) {
      for (const el of document.querySelectorAll(selector)) {
        const text = readText(el);
        if (text.length >= 10) return text.slice(0, 6000);
      }
    }
    // 语义兜底：不依赖 class，取最长的那块 article/main
    let best = '';
    for (const el of document.querySelectorAll('article, main article, [role="main"]')) {
      const text = readText(el);
      if (text.length > best.length) best = text;
    }
    return best.length >= 40 ? best.slice(0, 6000) : '';
  };
  const pickComments = (selectors, max) => {
    const out = [];
    const seen = new Set();
    for (const selector of selectors) {
      for (const el of document.querySelectorAll(selector)) {
        const text = readText(el);
        if (text.length < 2 || seen.has(text)) continue;
        seen.add(text);
        out.push(text.slice(0, 500));
        if (out.length >= max) return out;
      }
    }
    return out;
  };

  let plan;
  if (onHost('x.com') || onHost('twitter.com')) {
    // 主推文 = 第一条 article；后续 article 视为可见回复
    const tweets = Array.from(document.querySelectorAll('article[data-testid="tweet"]'))
      .map((a) => readText(a.querySelector('[data-testid="tweetText"]')))
      .filter((t) => t);
    const body = tweets[0] || readText(document.querySelector('article[role="article"]')) || '';
    const comments = tweets
      .slice(1)
      .filter((t) => t !== body)
      .slice(0, 5)
      .map((t) => t.slice(0, 500));
    return { body: body.slice(0, 6000), comments };
  }
  if (onHost('zhihu.com')) {
    plan = {
      body: ['.RichContent-inner', '.Post-RichTextContainer', '.RichText', 'article .RichText'],
      comments: ['.CommentContent', '.CommentItemV2 .content', '.Comments-container .content'],
    };
  } else if (onHost('xiaohongshu.com')) {
    plan = {
      body: ['#detail-desc', '.note-text', '.desc', '.note-content'],
      comments: ['.comment-item .content', '.comments-container .content', '.comment-item .note-text'],
    };
  } else if (onHost('douyin.com')) {
    plan = {
      body: ['[data-e2e="video-desc"]', '[data-e2e="video-desc-container"]', '.video-info-detail'],
      comments: ['[data-e2e="comment-item"]', '[class*="comment-item"] [class*="content"]'],
    };
  } else {
    throw new Error('Detail extractor does not match the current platform');
  }
  return { body: pickBody(plan.body), comments: pickComments(plan.comments, 5) };
};
