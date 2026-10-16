import { useEffect, useState } from 'react';
import { Globe } from 'lucide-react';
import { browserSiteIcon, browserSiteOrigin } from './browserSiteIcons';

export default function BrowserSiteIcon({ url, size = 20 }: { url: string; size?: number }) {
  const origin = browserSiteOrigin(url);
  const [image, setImage] = useState<{ origin: string; src: string } | null>(null);
  useEffect(() => {
    if (!origin) return;
    let active = true;
    void browserSiteIcon(origin).then(src => { if (active) setImage(src ? { origin, src } : null); });
    return () => { active = false; };
  }, [origin]);
  return image?.origin === origin
    ? <img className="embedded-browser-site-image" src={image.src} width={size} height={size} alt="" aria-hidden="true" draggable={false} onError={() => setImage(null)} />
    : <Globe size={size} aria-hidden="true" />;
}
