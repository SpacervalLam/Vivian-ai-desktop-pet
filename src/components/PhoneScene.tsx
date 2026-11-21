export function PhoneLockClock({ time }: { time: string }) {
  return <time className="phone-home-time" dateTime={time} aria-label={time}>
    <svg viewBox="0 22 320 96" preserveAspectRatio="none" aria-hidden="true">
      <defs>
        <linearGradient id="phone-clock-glass" x1="0" y1="0" x2=".55" y2="1">
          <stop offset="0" stopColor="#fff" stopOpacity=".97" /><stop offset=".34" stopColor="#fbf9ff" stopOpacity=".82" />
          <stop offset=".66" stopColor="#fff6e6" stopOpacity=".55" /><stop offset="1" stopColor="#dfe2f2" stopOpacity=".24" />
        </linearGradient>
        <linearGradient id="phone-clock-rim" x1="0" y1="0" x2="1" y2="1">
          <stop stopColor="#fff" stopOpacity=".85" /><stop offset=".38" stopColor="#fff" stopOpacity=".12" /><stop offset=".7" stopColor="#fff" stopOpacity=".65" /><stop offset="1" stopColor="#fff" stopOpacity=".1" />
        </linearGradient>
        <filter id="phone-clock-glow" x="-8%" y="-24%" width="116%" height="150%">
          <feGaussianBlur stdDeviation="2.6" />
        </filter>
      </defs>
      <text className="phone-clock-digits" x="160" y="114" textAnchor="middle" textLength="314" lengthAdjust="spacingAndGlyphs" fill="url(#phone-clock-glass)" filter="url(#phone-clock-glow)" opacity=".42">{time}</text>
      <text className="phone-clock-digits" x="160" y="114" textAnchor="middle" textLength="314" lengthAdjust="spacingAndGlyphs" fill="url(#phone-clock-glass)" stroke="url(#phone-clock-rim)" strokeWidth=".35">{time}</text>
    </svg>
  </time>;
}
