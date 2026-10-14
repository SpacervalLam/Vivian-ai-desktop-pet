import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { comicKeyForText } from '../../../i18n/comic';
import './ComicChapters.css';

/** Live controls remain real DOM; only the surrounding panel is illustrated. */
export function ComicChapterHeading({ title, number, caption, image, position = '50% 12%', children }: {
  title: string;
  number: string;
  caption: string;
  image: string;
  position?: string;
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  const translate = (value: string) => { const key = comicKeyForText(value); return key ? t(`comic.${key}`) : value; };
  return <header className="comic-chapter-heading">
    <div className="comic-chapter-shot" aria-hidden="true">
      <img src={`/mind-comic/${image}.webp`} alt="" style={{ objectPosition: position }} draggable={false} />
      <span className="comic-chapter-ink">{translate(caption)}</span>
    </div>
    <div className="comic-chapter-title-panel">
      <span className="comic-chapter-number">{t('comic.chapter', { n: number })}</span>
      <h2>{translate(title)}</h2>
      {children && <div className="comic-chapter-tools">{children}</div>}
    </div>
  </header>;
}

export function ComicChapterInset({ image, caption, position = '50% 35%' }: {
  image: string; caption: string; position?: string;
}) {
  const { t } = useTranslation();
  const key = comicKeyForText(caption);
  return <figure className="comic-chapter-inset">
    <img src={`/mind-comic/${image}.webp`} alt="" style={{ objectPosition: position }} draggable={false} />
    <figcaption>{key ? t(`comic.${key}`) : caption}</figcaption>
  </figure>;
}
