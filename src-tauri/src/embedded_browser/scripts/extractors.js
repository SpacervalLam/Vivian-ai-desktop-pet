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
