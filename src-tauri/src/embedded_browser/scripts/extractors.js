// Packaged DOM extractors for the native browser isolated world; no arbitrary code.
// DOM and URLs are untrusted page data; callers wrap results before returning them.
globalThis.vivianExtractPlatform = function (platform) {
  const plans = {
    bilibili: { host: 'bilibili.com', selector: 'a[href*="/video/BV"]', pattern: /\/video\/(BV[0-9A-Za-z]+)/ },
    xiaohongshu: { host: 'xiaohongshu.com', selector: 'a[href*="/explore/"], a[href*="/search_result/"]', pattern: /\/(?:explore|search_result)\/([0-9a-f]{8,})/ },
    douyin: { host: 'douyin.com', selector: 'a[href*="/video/"]', pattern: /\/video\/(\d+)/ },
    zhihu: { host: 'zhihu.com', selector: 'a[href*="/question/"], a[href*="/p/"]', pattern: /\/(?:question|p)\/(\d+)/ },
    weibo: { host: 'weibo.com', selector: 'a[href]', pattern: /^\/(?:\d+\/([A-Za-z0-9]+)|detail\/(\d+))\/?$/ },
    twitter: { host: 'x.com', aliases: ['twitter.com'], selector: 'a[href*="/status/"]', pattern: /\/status\/(\d+)/ },
    reddit: { host: 'reddit.com', selector: 'a[href*="/comments/"]', pattern: /\/comments\/([A-Za-z0-9]+)/ },
    github: { host: 'github.com', selector: 'a[href]', pattern: /^\/([^/]+\/[^/]+(?:\/(?:issues|pull|discussions)\/\d+)?)\/?$/ },
    arxiv: { host: 'arxiv.org', selector: 'a[href*="/abs/"]', pattern: /\/abs\/(\d{4}\.\d+(?:v\d+)?|[a-z-]+\/\d+(?:v\d+)?)/i },
    youtube: { host: 'youtube.com', selector: 'a[href*="/watch?"], a[href*="/shorts/"]', pattern: /\/shorts\/([A-Za-z0-9_-]+)/ },
  };
  const plan = plans[platform];
  if (!plan) throw new Error('Unsupported extractor: ' + platform);
  const onHost = (host) => [plan.host, ...(plan.aliases || [])].some(domain => host === domain || host.endsWith('.' + domain));
  if (!onHost(location.hostname)) throw new Error('Extractor does not match the current platform');
  const items = new Map();
  for (const anchor of document.querySelectorAll(plan.selector)) {
    let url;
    try { url = new URL(anchor.getAttribute('href'), location.href); } catch (_) { continue; }
    if (!['http:', 'https:'].includes(url.protocol) || !onHost(url.hostname) || url.username || url.password) continue;
    const match = url.pathname.match(plan.pattern);
    const id = platform === 'youtube' && url.pathname === '/watch'
      ? url.searchParams.get('v')?.match(/^[A-Za-z0-9_-]{11}$/)?.[0]
      : match?.slice(1).find(Boolean);
    if (!id) continue;
    if (platform === 'github' && /^(search|topics|collections|marketplace|settings|orgs|users|features|sponsors)\//.test(id)) continue;
    // Many feeds use separate cover/title anchors inside the same card.
    const card = anchor.closest('article, .note-item, .bili-video-card, .HotItem, .card-wrap, shreddit-post, ytd-video-renderer, li.arxiv-result, section') || anchor;
    const image = card.querySelector('img');
    const contentTitle = platform === 'twitter' ? card.querySelector('[data-testid="tweetText"]')?.textContent
      : platform === 'arxiv' ? card.querySelector('.title')?.textContent : '';
    const title = (contentTitle || anchor.getAttribute('title') || anchor.textContent?.trim()
      || card.querySelector('.title, h2, h3, [data-testid="tweetText"], .txt, #video-title')?.textContent || image?.alt || '').trim().slice(0, 160);
    if (!title) continue;
    const previous = items.get(id);
    // Prefer a title anchor, but preserve access tokens from the cover URL.
    if (previous) {
      if (!new URL(previous.url).search && url.search) previous.url = url.href;
      continue;
    }
    const description = (card.querySelector('.abstract, .desc, .txt, [data-testid="tweetText"], .search-title')?.textContent || title).trim().slice(0, 1000);
    items.set(id, { id, title, desc: description, url: url.href,
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
  if (/\/(?:login|signin|passport)(?:\/|$)|\/i\/flow\/login/.test(location.pathname)) {
    throw new Error('Login page: complete sign-in manually in the browser');
  }
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
  } else if (onHost('bilibili.com')) {
    plan = { body: ['#v_desc', '.basic-desc-info', '.video-desc', 'article'], comments: ['.reply-content', '.root-reply .reply-content-container'] };
  } else if (onHost('weibo.com')) {
    plan = { body: ['[class*="detail_wbtext"]', '.WB_text', '.txt', 'article'], comments: ['.WB_text [node-type="replywrap"]', '[class*="comment_text"]'] };
  } else if (onHost('reddit.com')) {
    plan = { body: ['[slot="text-body"]', '.usertext-body', 'shreddit-post'], comments: ['shreddit-comment [slot="comment"]', '.comment .md'] };
  } else if (onHost('github.com')) {
    plan = { body: ['article.markdown-body', '.markdown-body', '[data-testid="issue-body"]'], comments: ['.timeline-comment .comment-body'] };
  } else if (onHost('arxiv.org')) {
    plan = { body: ['blockquote.abstract', '.abstract', 'article'], comments: [] };
  } else if (onHost('youtube.com')) {
    plan = { body: ['#description-inline-expander', '#description', 'ytd-text-inline-expander'], comments: ['ytd-comment-thread-renderer #content-text'] };
  } else {
    throw new Error('Detail extractor does not match the current platform');
  }
  return { body: pickBody(plan.body), comments: pickComments(plan.comments, 5) };
};
