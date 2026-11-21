export function ClockAppArtwork() {
  return <svg viewBox="0 0 60 60" aria-hidden="true">
    {Array.from({ length: 60 }, (_, index) => <path key={index} d={index % 5 ? 'M30 4v2' : 'M30 3v4'} transform={`rotate(${index * 6} 30 30)`} stroke={index % 5 ? '#b5b4b4' : '#47474b'} strokeWidth={index % 5 ? '.55' : '.85'} />)}
    <g fill="#222225" fontSize="9" fontWeight="600" textAnchor="middle" fontFamily="Arial,sans-serif"><text x="30" y="15">12</text><text x="48" y="33">3</text><text x="30" y="51">6</text><text x="12" y="33">9</text></g>
    <path d="M30 15v15l10 9" fill="none" stroke="#151517" strokeWidth="2" strokeLinecap="round" /><path d="M30 34V9" stroke="#e2a44b" strokeWidth=".7" transform="rotate(-45 30 30)" /><circle cx="30" cy="30" r="1.6" fill="#222" />
  </svg>;
}
export function PhotosAppArtwork() {
  return <svg viewBox="0 0 60 60" aria-hidden="true">{['#f9d631','#f6a62e','#f77573','#ed6dac','#b17ac4','#709bdf','#57c6cc','#84ca52'].map((color, index) => <ellipse key={color} cx="30" cy="18" rx="8.1" ry="12.7" transform={`rotate(${index * 45} 30 30)`} fill={color} opacity=".83" style={{ mixBlendMode: 'multiply' }} />)}</svg>;
}
export function LockScreenFlashlight() {
  return <svg width="27" height="31" viewBox="0 0 28 32" aria-hidden="true"><path fill="currentColor" fillRule="evenodd" d="M7 1h14a2 2 0 0 1 2 2v2H5V3a2 2 0 0 1 2-2ZM5 7h18v4l-5 7v11a2 2 0 0 1-2 2h-4a2 2 0 0 1-2-2V18l-5-7V7Zm9 13a2 2 0 0 0-2 2v4a2 2 0 0 0 4 0v-4a2 2 0 0 0-2-2Z" /></svg>;
}
export function LockScreenCamera() {
  return <svg width="28" height="27" viewBox="0 0 32 28" aria-hidden="true"><path fill="currentColor" fillRule="evenodd" d="M11 2h10l3 4h4a3 3 0 0 1 3 3v15a3 3 0 0 1-3 3H4a3 3 0 0 1-3-3V9a3 3 0 0 1 3-3h4l3-4Zm5 7a7 7 0 1 0 0 14 7 7 0 0 0 0-14Zm0 3a4 4 0 1 1 0 8 4 4 0 0 1 0-8ZM26 9a1 1 0 1 0 0 2 1 1 0 0 0 0-2Z" /></svg>;
}
