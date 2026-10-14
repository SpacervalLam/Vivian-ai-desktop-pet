import { selectComicDialogue } from './comicDialogue.js';
import { comicText } from '../../../i18n/comic.js';

// Each spread is a fragment of their daily life. Live information deliberately
// crosses character pairs; persistent functional owners still live in the book.
export const comicTitles = ['Vivian & Nana', '在雨里找到你', '心情的颜色', '把今天留下', '一点点靠近', '约定', '字里行间', '留下的足迹', '电车里', '清晨'];

export function comicMarkup(data, page, assetBase, visit = 0, functional = false, language = 'zh-CN') {
  const esc = s => String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const rectangle = [[0,0],[100,0],[100,100],[0,100]];
  const position = (r,m) => `data-r="${r.join(',')}" data-m="${m.join(',')}"`;
  function shot(id, asset, r, m, order = 0, points = rectangle, cls = '', extra = '', focus = '50% 50%') {
    const phone = id === 'line-phone' ? `<img class="phone-screen-on" src="${assetBase}line-phone-on-v33.webp" alt="" aria-hidden="true">` : '';
    const hand = id === 'skirt-hand' ? `<div class="hand-frames" aria-hidden="true" style="background-image:url(${assetBase}vivian-hand-tremble-sheet-v26.webp)"></div>` : '';
    return `<div class="panel-shadow" ${position(r,m)}><div class="shot ${cls}" id="${id}" data-points='${JSON.stringify(points)}' style="--delay:${order*460}ms;--focus:${focus}"><div class="image-wrap"><img src="${assetBase}${asset}.webp" data-i18n-alt="comic.alt_${id}" alt="${esc(comicText(`alt_${id}`, language))}">${phone}${hand}</div>${extra}<svg class="edge" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true"><polygon points="${points.map(p => p.join(',')).join(' ')}"/></svg></div></div>`;
  }
  const box = (cls,r,m,html,order=0,key) => `<div class="${cls}" ${key ? `data-i18n="comic.${key}"` : ''} ${position(r,m)} style="--delay:${order*460}ms">${key ? esc(comicText(key,language)) : html}</div>`;
  const text = key => `<span data-i18n="comic.${key}" style="white-space:pre-line">${esc(comicText(key,language))}</span>`;
  const voice = (who,r,m,html,order=1,cls='') => {
    const cloud = cls.includes('cloud');
    const outline = cloud
      ? 'M 9,30 C -3,19 9,5 22,10 C 26,-2 42,-2 49,6 C 61,-3 77,0 79,10 C 95,3 105,19 94,30 C 106,41 103,60 95,66 C 104,81 92,94 79,89 C 73,103 57,104 49,95 C 36,104 21,99 20,90 C 4,97 -3,80 7,68 C -4,57 -2,39 9,30 Z'
      : 'M 50,1 C 81,0 99,14 99,48 C 100,82 81,99 50,99 C 18,99 1,83 1,50 C 0,18 18,1 50,1 Z';
    const tail = cls.includes('tail-right') ? 'M 98,43 L 112,35 L 99,56'
      : cls.includes('tail-left') ? 'M 2,43 L -11,35 L 1,57'
      : cls.includes('tail-up-right') ? 'M 69,4 L 84,-10 L 80,12'
      : 'M 20,12 L 17,-10 L 32,4';
    return box(`voice drawn-bubble ${who === 'nana' ? 'n' : 'v'} ${cls}`,r,m,`<svg class="bubble-outline" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true"><path d="${outline}"/>${cloud ? '' : `<path class="bubble-tail" d="${tail}"/>`}</svg>${html}`,order);
  };
  const sound = (value,r,m,order=2,cls='') => box(`comic-sfx ${cls}`,r,m,`<span aria-hidden="true">${text(({ '叮铃铃…':'alarm', '怦…':'heartbeat', '咔 哒':'trainSound' })[value])}</span>`,order);
  const thought = who => `<p data-bind="thought:${who}">${esc(data[who].thought)}</p>`;
  const line = (who,category) => `<p data-bind="dialogue:${who}:${category}">${esc(selectComicDialogue(who,category,data[who],visit,language))}</p>`;
  const presence = who => `<span class="presence" data-bind="presence:${who}">${esc(data[who].name)} · ${esc(data[who].presence)}</span>`;
  const caption = (value,r,m,order=1,vertical=false) => box(`narration ink-inset${vertical ? ' vertical' : ''}`,r,m,esc(value),order,'promiseCaption');
  const title = (value,r,m,kicker,order=0) => box('page-title chapter-title',r,m,`<small>${text(({ 'OUR EVERYDAY / 01':'everydayKicker', 'FEELINGS / 03':'feelingsKicker', 'LITTLE PLANS / 06':'plansKicker', 'TO BE CONTINUED / 08':'unfinishedKicker' })[kicker])}</small>${value}`,order);
  const goal = who => `<p data-bind="goal:${who}">${esc(data[who].goal)}</p>`;

  // Page 02 in the book: retain the supplied three-panel layout and open space.
  if (page === 9) {
    const frame = (x,y,w,h) => [x/2469*100,y/1372*100,w/2469*100,h/1372*100];
    const alarm = frame(1172,50,683,612);
    const curtains = frame(1876,50,533,1277);
    const waking = frame(91,706,1764,621);
    const ringing = frame(1090,585,680,125);
    return shot('morning-curtains','vivian-nana-day/page-01-morning-curtains-v02',curtains,curtains,0,rectangle,'morning-panel')
      + shot('morning-alarm','vivian-nana-day/page-01-alarm-clock-v02',alarm,alarm,1,rectangle,'morning-panel')
      + sound('叮铃铃…',ringing,ringing,0,'morning-alarm-sfx')
      + shot('morning-waking','vivian-nana-day/page-01-vivian-waking-v01',waking,waking,2,rectangle,'morning-panel');
  }

  // Keep historical scene indices stable, but never render the retired cover.
  if (page === 0) return '';

  if (page === 1) return shot('platform-light','vivian-nana-day/page-04-vivian-searching-v03',[0,0,58,62],[0,0,100,42],0,[[0,0],[100,0],[95.6976276,100],[0,100]],'rain-search-panel','','30% 0%')
    + box('narration boxed-narration',[3,3,38,0],[3,3,70,0],'',1,'rainArrival')
    + shot('rain-soaked-detail','vivian-nana-day/page-04-nana-rain-soaked-v02',[0,65,55.38388,35],[0,45,100,18],3,[[0,0],[100,0],[97.456565,100],[0,100]],'rain-detail-panel','','50% 50%')
    + shot('waiting-n','nana-umbrella-v02',[57,0,43,100],[0,66,100,34],6,[[9.36,0],[100,0],[100,100],[0,100]],'mirror','','50% 40%')
    + box('narration vertical rain-poem',[1,67,5,31],[1,46,7,16],'',4,'rainPoem')
    + voice('nana',[65,71,32,25],[47,79,50,19],`<small data-i18n="comic_nana_current_thought">${esc(comicText('nanaThoughtLabel',language))}</small>${thought('nana')}`,8,'compact cloud current-thought');

  if (page === 2) return shot('ripple','rain-clearing-v14',[0,0,100,20],[0,0,50,14],0,[[0,0],[100,0],[100,100],[0,100]])
    + title(text('feelings'),[54,2,42,15],[54,0,46,17],'FEELINGS / 03')
    + shot('emotion-n','nana-nai-face-adjusted-v19',[0,23,51,77],[0,19,100,36],2.25,[[0,0],[100,0],[93.95686,100],[0,100]],'','','54% 25%')
    + shot('emotion-v','vivian-rain-canonical-v14',[49.918,23,50.082,77],[0,59,100,38],1,[[6.153912,0],[100,0],[100,100],[0,100]],'','','48% 24%')
    + voice('nana',[3,67,37,28],[42,31,55,22],line('nana','emotions'),3,'compact cloud tail-up-left')
    + voice('vivian',[61,76,36,23],[41,73,56,24],line('vivian','emotions'),4,'compact tail-up-left')
    + sound('怦…',[44.4793,44,13,9],[75,55,20,8],5,'gutter-sound');

  if (page === 3) return shot('cafe-exterior','vivian-nana-day/cafe-rain-exterior-v02',[0,0,46,49],[0,0,100,24],0,rectangle,'cafe-establishing','','50% 50%')
    + shot('cafe-interior','vivian-nana-day/duo-window-handdrawn-v02',[0,51,46,49],[0,27,100,24],1150/460,rectangle,'','','50% 45%')
    + (functional ? '' : box('margin-line',[52,10,44,80],[3,57,94,40],text('diaryCaption'),2300/460));

  if (page === 4) return shot('hands','shared-handle-edit-v14a',[0,0,65,43],[0,13,100,27],4,[[0,0],[100,0],[97.354,100],[0,100]])
    + shot('together','shared-umbrella-landscape-v41',[63,0,37,100],[58,43,42,57],0,[[10.81,0],[100,0],[100,100],[0,100]])
    + shot('door-smile','shared-handle-corrected-v46',[0,46,63.16,54],[0,70,28,30],5,[[0,0],[100,0],[96.5801,100],[0,100]],'','','50% 50%');

  if (page === 5) return shot('letter-desk','notebook-letter-v31',[67,0,33,100],[61,21,39,28],0,[[12.12,0],[100,0],[100,100],[0,100]],'','','55% 48%')
    + box('narration ink-inset continued-caption',[70,90,28,8],[56,91,42,8],'',2,'toBeContinued')
    + (functional ? '' : title(text('plansCaption'),[3,30,56,57],[3,54,94,45],'LITTLE PLANS / 06'));

  if (page === 6) return shot('line-v','vivian-nana-day/page-03-vivian-phone-pajamas-v01',[0,0,43,100],[0,0,100,52],0,[[0,0],[100,0],[90.697674,100],[0,100]],'','','42% 32%')
    + box('narration boxed-narration time-caption',[2,3,4.8,12],[3,3,8,10],'',1,'morning')
    + shot('line-phone','line-phone-off-v33',[83,58,17,42],[80,34,20,20],2)
    + voice('vivian',[47,65,30,27],[0,37,65,19],`<small data-i18n="comic_vivian_current_thought">${esc(comicText('vivianThoughtLabel',language))}</small>${thought('vivian')}`,3,'compact cloud tail-left current-thought')
    + (functional ? '' : box('line-chat',[47,3,53,49],[3,57,94,40],`<p class="sent">${esc(data.vivian.thought)}</p><p class="received">${esc(data.nana.thought)}</p>`,1));

  if (page === 8) return shot('daily-arrival','train-arrival-v20',[0,0,66,44],[0,0,100,26],0,[[0,0],[100,0],[97.333333,100],[0,100]],'','','54% 48%')
    // Each entrance waits for the preceding one: train (1150ms), sound
    // (450ms), straps, Vivian, tickets, then the typed closing caption.
    + shot('daily-straps','train-straps-v20',[0,47,31,53],[0,29,48,31],1600/460,[[0,0],[100,0],[93.16129,100],[0,100]],'','','26% 35%')
    + shot('daily-pole','vivian-train-pole-v23',[64,0,36,100],[52,29,48,71],2750/460,[[11.111111,0],[100,0],[100,100],[0,100]],'',presence('vivian'),'46% 35%')
    + shot('daily-tickets','train-tickets-v20',[30.88,47,33.24,53],[0,63,48,37],3900/460,[[6.377858,0],[100,0],[93.622142,100],[0,100]],'','','22% 63%')
    + caption('把约定带在身边。',[55,53,5,38],[37,67,7,29],5050/460,true)
    + sound('咔 哒',[42,39,17,9],[29,24,23,7],1150/460,'late-sound');

  // The full drawing and six-frame detail retain their shared corner geometry.
  const rightTop = 11.4286, rightBottom = .2968;
  // Convert the right panel's horizontal drift into the wider middle panel's
  // coordinates: both span the full canvas height, so their edges stay parallel.
  const middleBottom = 100 + (rightBottom - rightTop) * 35 / 36.5;
  return shot('skirt-full','vivian-skirt-hand-v25',[30.5,0,36.5,100],functional?[50,0,50,43]:[24,0,76,43],2.5,[[0,0],[100,0],[middleBottom,100],[0,100]],'full-drawing','','50% 50%')
    + shot('hold-n','nana-umbrella-hold-v25',[65,0,35,100],[50,47,50,53],0,[[rightTop,0],[100,0],[100,100],[rightBottom,100]],'','','50% 22%')
    + shot('skirt-hand','vivian-skirt-hand-v25',[0,65,37,35],[0,66,44,34],4,rectangle,'hand-inset','','50% 83%')
    + voice('nana',[42,78,54,21],[0,46,45,18],line('nana','needs'),10,'tail-to-nana cloud')
    + (functional ? '' : title(text('unfinished'),[1,7,26,42],[0,0,46,37],'TO BE CONTINUED / 08'));
}
