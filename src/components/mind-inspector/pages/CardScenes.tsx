/**
 * 世界状态卡片场景装饰
 *
 * 纯内联 SVG 插画，根据卡片数据动态选择场景。
 * 京阿尼《小城日常》温暖柔和画风，半透明轻点缀。
 */

import React from 'react';

// === 共享容器 ===

const SceneWrap: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <svg
    viewBox="0 0 200 120"
    preserveAspectRatio="xMidYMax slice"
    style={{
      position: 'absolute',
      inset: 0,
      width: '100%',
      height: '100%',
      pointerEvents: 'none',
    }}
  >
    {children}
  </svg>
);

// === 时段场景（时间卡片） ===

export const TimeScene: React.FC<{ hour: number }> = ({ hour }) => {
  if (hour < 6) {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-t-ln" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#0f0f2d" />
            <stop offset="100%" stopColor="#1a1a3e" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-t-ln)" />
        {/* 弯月 */}
        <path d="M155,22 A14,14 0 1,0 155,50 A9,9 0 1,1 155,22" fill="#F5E6A3" opacity="0.25" />
        {/* 星星 */}
        <circle cx="30" cy="18" r="1.5" fill="#E8E0FF" opacity="0.35" />
        <circle cx="70" cy="32" r="1" fill="#E8E0FF" opacity="0.25" />
        <circle cx="110" cy="14" r="1.2" fill="#E8E0FF" opacity="0.3" />
        <circle cx="175" cy="40" r="1" fill="#E8E0FF" opacity="0.2" />
        <circle cx="50" cy="50" r="0.8" fill="#E8E0FF" opacity="0.2" />
        <circle cx="140" cy="55" r="1" fill="#E8E0FF" opacity="0.15" />
      </SceneWrap>
    );
  }

  if (hour < 12) {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-t-m" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#FFF8E7" />
            <stop offset="60%" stopColor="#FFECD2" />
            <stop offset="100%" stopColor="#FCB69F" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-t-m)" />
        {/* 晨光 */}
        <path d="M160,120 L130,50 L175,70 Z" fill="#FFD700" opacity="0.06" />
        <path d="M170,120 L155,35 L190,65 Z" fill="#FFD700" opacity="0.04" />
        {/* 太阳 */}
        <circle cx="170" cy="100" r="22" fill="#FFD93D" opacity="0.2" />
        <circle cx="170" cy="100" r="12" fill="#FFE066" opacity="0.25" />
        {/* 远景建筑 */}
        <rect x="5" y="95" width="16" height="25" rx="2" fill="#C9A96E" opacity="0.1" />
        <rect x="25" y="88" width="12" height="32" rx="2" fill="#C9A96E" opacity="0.08" />
        <rect x="42" y="92" width="20" height="28" rx="2" fill="#C9A96E" opacity="0.1" />
        <rect x="68" y="85" width="14" height="35" rx="2" fill="#C9A96E" opacity="0.07" />
        <rect x="88" y="95" width="10" height="25" rx="1" fill="#C9A96E" opacity="0.06" />
      </SceneWrap>
    );
  }

  if (hour < 18) {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-t-a" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#E0F2FE" />
            <stop offset="70%" stopColor="#BAE6FD" />
            <stop offset="100%" stopColor="#FEF3C7" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-t-a)" />
        {/* 太阳 */}
        <circle cx="160" cy="35" r="16" fill="#FCD34D" opacity="0.18" />
        <circle cx="160" cy="35" r="24" fill="#FCD34D" opacity="0.06" />
        {/* 云朵 */}
        <g opacity="0.15">
          <ellipse cx="45" cy="40" rx="22" ry="10" fill="white" />
          <ellipse cx="35" cy="36" rx="14" ry="8" fill="white" />
          <ellipse cx="58" cy="38" rx="12" ry="7" fill="white" />
        </g>
        <g opacity="0.1">
          <ellipse cx="130" cy="55" rx="18" ry="8" fill="white" />
          <ellipse cx="120" cy="52" rx="12" ry="6" fill="white" />
        </g>
      </SceneWrap>
    );
  }

  if (hour < 21) {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-t-e" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#F97316" stopOpacity="0.5" />
            <stop offset="40%" stopColor="#EC4899" stopOpacity="0.3" />
            <stop offset="100%" stopColor="#6366F1" stopOpacity="0.2" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="#FFF7ED" />
        <rect width="200" height="120" fill="url(#cs-t-e)" />
        {/* 远山 */}
        <path d="M-5,120 L-5,80 Q30,55 65,78 Q100,52 135,75 Q170,50 205,72 L205,120 Z" fill="#6D28D9" opacity="0.1" />
        <path d="M-5,120 L-5,92 Q45,72 90,88 Q135,68 205,85 L205,120 Z" fill="#4C1D95" opacity="0.08" />
        {/* 晚霞太阳 */}
        <circle cx="100" cy="62" r="18" fill="#FBBF24" opacity="0.15" />
        <circle cx="100" cy="62" r="30" fill="#FBBF24" opacity="0.06" />
      </SceneWrap>
    );
  }

  // Night (21-23)
  return (
    <SceneWrap>
      <defs>
        <linearGradient id="cs-t-n" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#1e1b4b" />
          <stop offset="100%" stopColor="#312e81" />
        </linearGradient>
      </defs>
      <rect width="200" height="120" fill="url(#cs-t-n)" />
      {/* 月亮 */}
      <circle cx="150" cy="30" r="16" fill="#FEF3C7" opacity="0.22" />
      <circle cx="157" cy="25" r="13" fill="#312e81" opacity="0.9" />
      {/* 星星 */}
      <circle cx="25" cy="20" r="1.5" fill="#FEF3C7" opacity="0.3" />
      <circle cx="60" cy="35" r="1" fill="#FEF3C7" opacity="0.25" />
      <circle cx="90" cy="15" r="1.3" fill="#FEF3C7" opacity="0.3" />
      <circle cx="45" cy="50" r="0.8" fill="#FEF3C7" opacity="0.2" />
      <circle cx="120" cy="45" r="1" fill="#FEF3C7" opacity="0.2" />
      <circle cx="180" cy="50" r="1.2" fill="#FEF3C7" opacity="0.25" />
      <circle cx="170" cy="18" r="0.8" fill="#FEF3C7" opacity="0.15" />
    </SceneWrap>
  );
};

// === 季节场景（季节卡片） ===

export const SeasonScene: React.FC<{ season: string }> = ({ season }) => {
  const s = season.toLowerCase();

  if (s === 'spring') {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-s-sp" x1="0" y1="0" x2="0.3" y2="1">
            <stop offset="0%" stopColor="#FDF2F8" />
            <stop offset="100%" stopColor="#D1FAE5" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-s-sp)" />
        {/* 嫩枝 */}
        <path d="M0,120 Q10,100 5,80" stroke="#86EFAC" strokeWidth="2" fill="none" opacity="0.15" />
        <path d="M5,80 Q15,70 8,55" stroke="#86EFAC" strokeWidth="1.5" fill="none" opacity="0.12" />
        <circle cx="8" cy="55" r="4" fill="#FBCFE8" opacity="0.2" />
        <circle cx="15" cy="70" r="3" fill="#FBCFE8" opacity="0.15" />
        {/* 樱花瓣飘落 */}
        <ellipse cx="40" cy="25" rx="4" ry="2.5" fill="#FBCFE8" opacity="0.22" transform="rotate(-30 40 25)" />
        <ellipse cx="80" cy="40" rx="3.5" ry="2" fill="#F9A8D4" opacity="0.18" transform="rotate(15 80 40)" />
        <ellipse cx="120" cy="20" rx="3" ry="2" fill="#FBCFE8" opacity="0.2" transform="rotate(-45 120 20)" />
        <ellipse cx="155" cy="45" rx="4" ry="2.5" fill="#F9A8D4" opacity="0.15" transform="rotate(25 155 45)" />
        <ellipse cx="60" cy="60" rx="3" ry="1.8" fill="#FBCFE8" opacity="0.12" transform="rotate(-10 60 60)" />
        <ellipse cx="170" cy="30" rx="3.5" ry="2" fill="#FBCFE8" opacity="0.18" transform="rotate(40 170 30)" />
        <ellipse cx="100" cy="55" rx="3" ry="2" fill="#F9A8D4" opacity="0.1" transform="rotate(-20 100 55)" />
      </SceneWrap>
    );
  }

  if (s === 'summer') {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-s-su" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#ECFDF5" />
            <stop offset="100%" stopColor="#A7F3D0" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-s-su)" />
        {/* 太阳 */}
        <circle cx="170" cy="25" r="18" fill="#FDE047" opacity="0.12" />
        {/* 大树 */}
        <rect x="145" y="75" width="6" height="45" rx="2" fill="#92400E" opacity="0.1" />
        <circle cx="148" cy="65" r="22" fill="#34D399" opacity="0.12" />
        <circle cx="135" cy="72" r="16" fill="#6EE7B7" opacity="0.1" />
        <circle cx="162" cy="70" r="14" fill="#34D399" opacity="0.08" />
        {/* 小树 */}
        <rect x="25" y="90" width="4" height="30" rx="1" fill="#92400E" opacity="0.08" />
        <circle cx="27" cy="82" r="14" fill="#6EE7B7" opacity="0.1" />
        {/* 蜻蜓 */}
        <g opacity="0.12" transform="translate(60,30)">
          <line x1="0" y1="0" x2="12" y2="0" stroke="#6B7280" strokeWidth="1" />
          <ellipse cx="3" cy="-3" rx="5" ry="2" fill="#93C5FD" opacity="0.6" />
          <ellipse cx="3" cy="3" rx="5" ry="2" fill="#93C5FD" opacity="0.6" />
        </g>
      </SceneWrap>
    );
  }

  if (s === 'autumn') {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-s-au" x1="0" y1="0" x2="0.2" y2="1">
            <stop offset="0%" stopColor="#FFF7ED" />
            <stop offset="100%" stopColor="#FED7AA" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-s-au)" />
        {/* 树干 */}
        <rect x="155" y="50" width="5" height="70" rx="2" fill="#92400E" opacity="0.1" />
        <path d="M157,50 Q140,40 130,55" stroke="#92400E" strokeWidth="2" fill="none" opacity="0.08" />
        <path d="M158,60 Q175,45 185,55" stroke="#92400E" strokeWidth="2" fill="none" opacity="0.08" />
        {/* 飘落的叶子 */}
        <ellipse cx="45" cy="25" rx="5" ry="3" fill="#F97316" opacity="0.2" transform="rotate(-35 45 25)" />
        <ellipse cx="90" cy="40" rx="4" ry="2.5" fill="#EF4444" opacity="0.15" transform="rotate(20 90 40)" />
        <ellipse cx="130" cy="20" rx="4.5" ry="2.5" fill="#F59E0B" opacity="0.18" transform="rotate(-15 130 20)" />
        <ellipse cx="70" cy="55" rx="3.5" ry="2" fill="#DC2626" opacity="0.12" transform="rotate(30 70 55)" />
        <ellipse cx="170" cy="35" rx="4" ry="2.5" fill="#F97316" opacity="0.15" transform="rotate(-25 170 35)" />
        <ellipse cx="25" cy="50" rx="3" ry="2" fill="#F59E0B" opacity="0.1" transform="rotate(45 25 50)" />
        <ellipse cx="110" cy="50" rx="4" ry="2" fill="#EA580C" opacity="0.13" transform="rotate(10 110 50)" />
      </SceneWrap>
    );
  }

  if (s === 'winter') {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-s-wi" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#F1F5F9" />
            <stop offset="100%" stopColor="#CBD5E1" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-s-wi)" />
        {/* 远山 */}
        <path d="M-5,120 L-5,85 Q30,60 70,82 Q110,55 150,78 Q180,60 205,75 L205,120 Z" fill="#94A3B8" opacity="0.1" />
        {/* 枯枝 */}
        <path d="M170,120 L170,60 Q165,45 155,50" stroke="#78716C" strokeWidth="2" fill="none" opacity="0.1" />
        <path d="M170,75 Q180,60 190,65" stroke="#78716C" strokeWidth="1.5" fill="none" opacity="0.08" />
        <path d="M170,85 Q160,75 150,80" stroke="#78716C" strokeWidth="1" fill="none" opacity="0.06" />
        {/* 雪花 */}
        <circle cx="30" cy="20" r="2.5" fill="white" opacity="0.25" />
        <circle cx="70" cy="35" r="2" fill="white" opacity="0.2" />
        <circle cx="110" cy="15" r="2.5" fill="white" opacity="0.22" />
        <circle cx="50" cy="50" r="1.8" fill="white" opacity="0.18" />
        <circle cx="140" cy="40" r="2" fill="white" opacity="0.2" />
        <circle cx="90" cy="55" r="1.5" fill="white" opacity="0.15" />
        <circle cx="180" cy="30" r="2" fill="white" opacity="0.18" />
        <circle cx="20" cy="45" r="1.5" fill="white" opacity="0.12" />
      </SceneWrap>
    );
  }

  return null;
};

// === 天气场景（天气卡片） ===

export const WeatherScene: React.FC<{ code?: number }> = ({ code }) => {
  if (code == null) return null;

  // 晴天
  if (code === 0) {
    return (
      <SceneWrap>
        <defs>
          <radialGradient id="cs-w-sun" cx="0.75" cy="0.25" r="0.6">
            <stop offset="0%" stopColor="#FEF3C7" />
            <stop offset="100%" stopColor="#BFDBFE" />
          </radialGradient>
        </defs>
        <rect width="200" height="120" fill="#EFF6FF" />
        <rect width="200" height="120" fill="url(#cs-w-sun)" opacity="0.5" />
        {/* 太阳光晕 */}
        <circle cx="155" cy="30" r="35" fill="#FCD34D" opacity="0.08" />
        <circle cx="155" cy="30" r="22" fill="#FBBF24" opacity="0.12" />
        <circle cx="155" cy="30" r="12" fill="#FDE047" opacity="0.18" />
        {/* 柔和光线 */}
        <line x1="155" y1="30" x2="120" y2="80" stroke="#FDE047" strokeWidth="1" opacity="0.06" />
        <line x1="155" y1="30" x2="100" y2="50" stroke="#FDE047" strokeWidth="1" opacity="0.05" />
        <line x1="155" y1="30" x2="140" y2="90" stroke="#FDE047" strokeWidth="1" opacity="0.04" />
      </SceneWrap>
    );
  }

  // 多云
  if (code <= 3) {
    return (
      <SceneWrap>
        <rect width="200" height="120" fill="#F0F9FF" />
        <g opacity="0.18">
          <ellipse cx="60" cy="45" rx="30" ry="14" fill="#CBD5E1" />
          <ellipse cx="45" cy="40" rx="20" ry="10" fill="#E2E8F0" />
          <ellipse cx="80" cy="42" rx="18" ry="9" fill="#E2E8F0" />
        </g>
        <g opacity="0.12">
          <ellipse cx="145" cy="60" rx="25" ry="12" fill="#CBD5E1" />
          <ellipse cx="132" cy="56" rx="16" ry="8" fill="#E2E8F0" />
          <ellipse cx="160" cy="57" rx="14" ry="7" fill="#E2E8F0" />
        </g>
        <g opacity="0.08">
          <ellipse cx="100" cy="80" rx="20" ry="10" fill="#CBD5E1" />
          <ellipse cx="88" cy="77" rx="14" ry="7" fill="#E2E8F0" />
        </g>
      </SceneWrap>
    );
  }

  // 雾
  if (code <= 48) {
    return (
      <SceneWrap>
        <rect width="200" height="120" fill="#F8FAFC" />
        <rect x="10" y="30" width="180" height="10" rx="5" fill="#94A3B8" opacity="0.08" />
        <rect x="20" y="50" width="160" height="8" rx="4" fill="#94A3B8" opacity="0.06" />
        <rect x="5" y="68" width="190" height="12" rx="6" fill="#94A3B8" opacity="0.1" />
        <rect x="30" y="88" width="140" height="8" rx="4" fill="#94A3B8" opacity="0.05" />
        <rect x="15" y="102" width="170" height="6" rx="3" fill="#94A3B8" opacity="0.04" />
      </SceneWrap>
    );
  }

  // 毛毛雨
  if (code <= 57) {
    return (
      <SceneWrap>
        <rect width="200" height="120" fill="#F0F9FF" />
        {/* 小云 */}
        <g opacity="0.1">
          <ellipse cx="80" cy="20" rx="25" ry="10" fill="#94A3B8" />
          <ellipse cx="65" cy="17" rx="15" ry="7" fill="#CBD5E1" />
        </g>
        {/* 细雨丝 */}
        {[20, 45, 70, 95, 120, 145, 170].map((x, i) => (
          <line key={i} x1={x} y1={30 + i * 3} x2={x - 3} y2={50 + i * 3} stroke="#93C5FD" strokeWidth="0.8" opacity="0.15" />
        ))}
      </SceneWrap>
    );
  }

  // 雨
  if (code <= 67 || (code >= 80 && code <= 82)) {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-w-rain" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#DBEAFE" />
            <stop offset="100%" stopColor="#BFDBFE" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-w-rain)" />
        {/* 乌云 */}
        <g opacity="0.12">
          <ellipse cx="70" cy="18" rx="35" ry="14" fill="#64748B" />
          <ellipse cx="50" cy="14" rx="22" ry="10" fill="#94A3B8" />
          <ellipse cx="95" cy="15" rx="18" ry="9" fill="#94A3B8" />
        </g>
        {/* 雨滴 */}
        {[15, 40, 65, 90, 115, 140, 165, 185].map((x, i) => (
          <line key={i} x1={x} y1={30 + (i % 3) * 8} x2={x - 4} y2={55 + (i % 3) * 8} stroke="#3B82F6" strokeWidth="1.2" opacity="0.12" strokeLinecap="round" />
        ))}
        {/* 水洼 */}
        <ellipse cx="60" cy="112" rx="18" ry="3" fill="#93C5FD" opacity="0.08" />
        <ellipse cx="140" cy="115" rx="14" ry="2.5" fill="#93C5FD" opacity="0.06" />
      </SceneWrap>
    );
  }

  // 雪
  if (code <= 77 || code >= 85) {
    return (
      <SceneWrap>
        <rect width="200" height="120" fill="#F8FAFC" />
        {/* 淡云 */}
        <g opacity="0.08">
          <ellipse cx="80" cy="15" rx="40" ry="12" fill="#CBD5E1" />
          <ellipse cx="150" cy="12" rx="30" ry="10" fill="#E2E8F0" />
        </g>
        {/* 雪花 */}
        {[
          [25, 30, 3], [55, 20, 2.5], [85, 40, 2], [115, 25, 3],
          [145, 45, 2.5], [175, 30, 2], [40, 55, 2], [100, 60, 2.5],
          [160, 55, 2], [70, 70, 1.8], [130, 70, 2.2],
        ].map(([cx, cy, r], i) => (
          <circle key={i} cx={cx} cy={cy} r={r} fill="white" opacity={0.15 + (i % 3) * 0.05} />
        ))}
      </SceneWrap>
    );
  }

  // 雷暴
  if (code >= 95) {
    return (
      <SceneWrap>
        <defs>
          <linearGradient id="cs-w-storm" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#374151" />
            <stop offset="100%" stopColor="#1F2937" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="#F3F4F6" />
        <rect width="200" height="120" fill="url(#cs-w-storm)" opacity="0.15" />
        {/* 乌云 */}
        <g opacity="0.15">
          <ellipse cx="100" cy="20" rx="50" ry="18" fill="#374151" />
          <ellipse cx="75" cy="15" rx="30" ry="12" fill="#4B5563" />
          <ellipse cx="130" cy="16" rx="25" ry="11" fill="#4B5563" />
        </g>
        {/* 闪电 */}
        <path d="M95,35 L85,60 L95,58 L82,85" stroke="#FBBF24" strokeWidth="2.5" fill="none" opacity="0.2" strokeLinecap="round" strokeLinejoin="round" />
        {/* 雨 */}
        {[30, 60, 130, 160].map((x, i) => (
          <line key={i} x1={x} y1={40} x2={x - 3} y2={65} stroke="#60A5FA" strokeWidth="1" opacity="0.1" />
        ))}
      </SceneWrap>
    );
  }

  return null;
};

// === 系统卡片装饰 ===

export const CpuScene: React.FC = () => (
  <SceneWrap>
    <rect width="200" height="120" fill="#F0FDF4" opacity="0.5" />
    {/* 柔和波浪 */}
    <path d="M0,90 Q25,75 50,90 Q75,105 100,90 Q125,75 150,90 Q175,105 200,90 L200,120 L0,120 Z" fill="#86EFAC" opacity="0.08" />
    <path d="M0,100 Q25,88 50,100 Q75,112 100,100 Q125,88 150,100 Q175,112 200,100 L200,120 L0,120 Z" fill="#4ADE80" opacity="0.06" />
    {/* 叶脉纹理 */}
    <path d="M170,120 Q175,95 165,80" stroke="#86EFAC" strokeWidth="1" fill="none" opacity="0.1" />
    <path d="M165,80 Q155,70 160,60" stroke="#86EFAC" strokeWidth="0.8" fill="none" opacity="0.08" />
    <path d="M165,90 Q175,82 180,75" stroke="#86EFAC" strokeWidth="0.8" fill="none" opacity="0.06" />
  </SceneWrap>
);

export const MemoryScene: React.FC = () => (
  <SceneWrap>
    <rect width="200" height="120" fill="#FFFBEB" opacity="0.3" />
    {/* 书本堆叠 */}
    <rect x="15" y="82" width="35" height="8" rx="2" fill="#F59E0B" opacity="0.12" />
    <rect x="12" y="74" width="38" height="8" rx="2" fill="#3B82F6" opacity="0.1" />
    <rect x="17" y="66" width="32" height="8" rx="2" fill="#EF4444" opacity="0.08" />
    {/* 盆栽 */}
    <rect x="160" y="92" width="14" height="16" rx="3" fill="#D97706" opacity="0.1" />
    <circle cx="167" cy="82" r="10" fill="#34D399" opacity="0.12" />
    <circle cx="160" cy="86" r="7" fill="#6EE7B7" opacity="0.08" />
    <rect x="166" y="82" width="2" height="10" rx="1" fill="#065F46" opacity="0.06" />
  </SceneWrap>
);

export const ForegroundScene: React.FC = () => (
  <SceneWrap>
    <rect width="200" height="120" fill="#FEF3C7" opacity="0.2" />
    {/* 窗台 */}
    <rect x="0" y="95" width="200" height="4" rx="1" fill="#D4A574" opacity="0.1" />
    {/* 咖啡杯 */}
    <rect x="155" y="78" width="16" height="17" rx="3" fill="#92400E" opacity="0.1" />
    <rect x="170" y="82" width="5" height="8" rx="2.5" fill="none" stroke="#92400E" strokeWidth="1.2" opacity="0.08" />
    {/* 蒸汽 */}
    <path d="M160,75 Q158,68 162,62" stroke="#94A3B8" strokeWidth="1" fill="none" opacity="0.08" />
    <path d="M165,76 Q167,70 164,64" stroke="#94A3B8" strokeWidth="0.8" fill="none" opacity="0.06" />
    {/* 铅笔 */}
    <rect x="20" y="88" width="40" height="4" rx="1" fill="#F59E0B" opacity="0.1" transform="rotate(-8 40 90)" />
    <path d="M18,87 L15,90 L20,92" fill="#2C2C2C" opacity="0.06" />
  </SceneWrap>
);

export const NetworkScene: React.FC = () => (
  <SceneWrap>
    <defs>
      <linearGradient id="cs-net" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0%" stopColor="#EFF6FF" />
        <stop offset="100%" stopColor="#DBEAFE" />
      </linearGradient>
    </defs>
    <rect width="200" height="120" fill="url(#cs-net)" opacity="0.4" />
    {/* 屋顶天际线 */}
    <path d="M0,120 L0,95 L15,85 L30,95 L35,88 L50,78 L65,88 L70,82 L85,75 L100,85 L105,90 L120,80 L135,90 L140,85 L160,75 L175,85 L180,90 L200,82 L200,120 Z" fill="#64748B" opacity="0.08" />
    {/* 电线杆 */}
    <rect x="40" y="50" width="3" height="70" rx="1" fill="#78716C" opacity="0.1" />
    <line x1="25" y1="55" x2="55" y2="55" stroke="#78716C" strokeWidth="0.8" opacity="0.08" />
    <line x1="28" y1="62" x2="52" y2="62" stroke="#78716C" strokeWidth="0.8" opacity="0.06" />
    {/* 电线 */}
    <path d="M43,55 Q100,48 155,55" stroke="#78716C" strokeWidth="0.6" fill="none" opacity="0.06" />
    <path d="M43,62 Q100,56 155,62" stroke="#78716C" strokeWidth="0.6" fill="none" opacity="0.05" />
    {/* 第二个电线杆 */}
    <rect x="155" y="52" width="3" height="68" rx="1" fill="#78716C" opacity="0.1" />
    <line x1="140" y1="57" x2="170" y2="57" stroke="#78716C" strokeWidth="0.8" opacity="0.08" />
  </SceneWrap>
);

export const LocationScene: React.FC = () => (
  <SceneWrap>
    <defs>
      <linearGradient id="cs-loc" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0%" stopColor="#ECFDF5" />
        <stop offset="100%" stopColor="#D1FAE5" />
      </linearGradient>
    </defs>
    <rect width="200" height="120" fill="url(#cs-loc)" opacity="0.3" />
    {/* 远山 */}
    <path d="M-5,90 Q30,55 70,78 Q110,50 150,72 Q180,55 205,68 L205,120 L-5,120 Z" fill="#6EE7B7" opacity="0.1" />
    {/* 田野 */}
    <path d="M0,120 L0,95 Q50,85 100,92 Q150,82 200,90 L200,120 Z" fill="#86EFAC" opacity="0.08" />
    {/* 田野线条 */}
    <line x1="0" y1="105" x2="200" y2="100" stroke="#34D399" strokeWidth="0.5" opacity="0.08" />
    <line x1="0" y1="112" x2="200" y2="108" stroke="#34D399" strokeWidth="0.5" opacity="0.06" />
    {/* 小房子 */}
    <rect x="130" y="78" width="12" height="10" rx="1" fill="#D4A574" opacity="0.1" />
    <path d="M128,78 L136,70 L144,78" fill="#EF4444" opacity="0.08" />
  </SceneWrap>
);

export const PresenceScene: React.FC<{ isPresent: boolean }> = ({ isPresent }) => (
  <SceneWrap>
    {isPresent ? (
      <>
        <rect width="200" height="120" fill="#F0FDF4" opacity="0.3" />
        {/* 阳光 */}
        <circle cx="160" cy="25" r="20" fill="#FDE047" opacity="0.08" />
        <circle cx="160" cy="25" r="30" fill="#FDE047" opacity="0.04" />
        {/* 盆栽 */}
        <rect x="25" y="88" width="18" height="22" rx="4" fill="#D97706" opacity="0.1" />
        <circle cx="34" cy="78" r="14" fill="#34D399" opacity="0.12" />
        <circle cx="28" cy="82" r="10" fill="#6EE7B7" opacity="0.08" />
        <rect x="33" y="78" width="2" height="10" rx="1" fill="#065F46" opacity="0.06" />
        {/* 小花 */}
        <circle cx="165" cy="95" r="3" fill="#FBCFE8" opacity="0.15" />
        <circle cx="175" cy="100" r="2.5" fill="#F9A8D4" opacity="0.12" />
        <circle cx="155" cy="102" r="2" fill="#FBCFE8" opacity="0.1" />
      </>
    ) : (
      <>
        <rect width="200" height="120" fill="#FFF7ED" opacity="0.3" />
        {/* 门框 */}
        <rect x="140" y="35" width="30" height="55" rx="3" fill="#D4A574" opacity="0.1" />
        <rect x="143" y="38" width="24" height="49" rx="2" fill="#FEF3C7" opacity="0.08" />
        <circle cx="163" cy="62" r="2" fill="#92400E" opacity="0.1" />
        {/* 小猫剪影 */}
        <g opacity="0.12" transform="translate(30,82)">
          {/* 身体 */}
          <ellipse cx="15" cy="15" rx="14" ry="10" fill="#78716C" />
          {/* 头 */}
          <circle cx="28" cy="10" r="8" fill="#78716C" />
          {/* 耳朵 */}
          <path d="M23,4 L25,0 L28,5" fill="#78716C" />
          <path d="M30,4 L32,0 L34,5" fill="#78716C" />
          {/* 尾巴 */}
          <path d="M2,12 Q-5,5 0,0" stroke="#78716C" strokeWidth="3" fill="none" strokeLinecap="round" />
        </g>
      </>
    )}
  </SceneWrap>
);

export const MusicScene: React.FC<{ isPlaying: boolean }> = ({ isPlaying }) => (
  <SceneWrap>
    {isPlaying ? (
      <>
        <defs>
          <linearGradient id="cs-music-play" x1="0" y1="0" x2="0.5" y2="1">
            <stop offset="0%" stopColor="#FDF2F8" />
            <stop offset="100%" stopColor="#ECFDF5" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-music-play)" opacity="0.4" />
        {/* 音符 */}
        <g opacity="0.15">
          <circle cx="35" cy="35" r="4" fill="#A78BFA" />
          <line x1="39" y1="35" x2="39" y2="18" stroke="#A78BFA" strokeWidth="1.5" />
          <path d="M39,18 Q45,15 45,20" stroke="#A78BFA" strokeWidth="1.5" fill="none" />
        </g>
        <g opacity="0.1">
          <circle cx="155" cy="45" r="3.5" fill="#F472B6" />
          <line x1="158.5" y1="45" x2="158.5" y2="30" stroke="#F472B6" strokeWidth="1.2" />
          <path d="M158.5,30 Q163,27 163,32" stroke="#F472B6" strokeWidth="1.2" fill="none" />
        </g>
        {/* 花丛 */}
        <circle cx="20" cy="108" r="4" fill="#FBCFE8" opacity="0.15" />
        <circle cx="32" cy="112" r="3" fill="#F9A8D4" opacity="0.12" />
        <circle cx="45" cy="110" r="3.5" fill="#FBCFE8" opacity="0.1" />
        <circle cx="165" cy="105" r="3" fill="#C4B5FD" opacity="0.12" />
        <circle cx="178" cy="110" r="4" fill="#A78BFA" opacity="0.1" />
      </>
    ) : (
      <>
        <rect width="200" height="120" fill="#F5F5F4" opacity="0.3" />
        {/* 安静的草地 */}
        <path d="M0,120 L0,100 Q50,90 100,98 Q150,88 200,95 L200,120 Z" fill="#BBF7D0" opacity="0.1" />
        <path d="M0,120 L0,108 Q50,100 100,106 Q150,98 200,105 L200,120 Z" fill="#86EFAC" opacity="0.06" />
        {/* 小草 */}
        <line x1="30" y1="105" x2="28" y2="95" stroke="#6EE7B7" strokeWidth="1" opacity="0.1" />
        <line x1="33" y1="106" x2="35" y2="97" stroke="#6EE7B7" strokeWidth="1" opacity="0.08" />
        <line x1="160" y1="100" x2="158" y2="90" stroke="#6EE7B7" strokeWidth="1" opacity="0.1" />
        <line x1="163" y1="101" x2="166" y2="92" stroke="#6EE7B7" strokeWidth="1" opacity="0.08" />
      </>
    )}
  </SceneWrap>
);

export const SunScene: React.FC<{ isDaytime: boolean }> = ({ isDaytime }) => (
  <SceneWrap>
    {isDaytime ? (
      <>
        <defs>
          <linearGradient id="cs-sun-day" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#FEF9C3" />
            <stop offset="100%" stopColor="#BFDBFE" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-sun-day)" opacity="0.4" />
        {/* 太阳升起 */}
        <circle cx="100" cy="50" r="20" fill="#FBBF24" opacity="0.15" />
        <circle cx="100" cy="50" r="35" fill="#FBBF24" opacity="0.06" />
        {/* 地平线 */}
        <line x1="0" y1="85" x2="200" y2="85" stroke="#F59E0B" strokeWidth="1" opacity="0.08" />
        {/* 地面 */}
        <rect x="0" y="85" width="200" height="35" fill="#D1FAE5" opacity="0.1" />
      </>
    ) : (
      <>
        <defs>
          <linearGradient id="cs-sun-night" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#312E81" />
            <stop offset="100%" stopColor="#1E1B4B" />
          </linearGradient>
        </defs>
        <rect width="200" height="120" fill="url(#cs-sun-night)" opacity="0.3" />
        {/* 月亮 */}
        <circle cx="100" cy="40" r="18" fill="#FEF3C7" opacity="0.2" />
        <circle cx="108" cy="34" r="15" fill="#312E81" />
        {/* 星星 */}
        <circle cx="30" cy="25" r="1.2" fill="#FEF3C7" opacity="0.3" />
        <circle cx="60" cy="15" r="1" fill="#FEF3C7" opacity="0.25" />
        <circle cx="150" cy="20" r="1.5" fill="#FEF3C7" opacity="0.28" />
        <circle cx="180" cy="35" r="1" fill="#FEF3C7" opacity="0.2" />
        <circle cx="45" cy="45" r="0.8" fill="#FEF3C7" opacity="0.15" />
        {/* 地平线 */}
        <line x1="0" y1="85" x2="200" y2="85" stroke="#4338CA" strokeWidth="1" opacity="0.1" />
      </>
    )}
  </SceneWrap>
);

export const VolumeScene: React.FC = () => (
  <SceneWrap>
    <rect width="200" height="120" fill="#FAFAF9" opacity="0.3" />
    {/* 音波 */}
    <path d="M85,60 Q90,40 95,60 Q100,80 105,60" stroke="#A78BFA" strokeWidth="1.5" fill="none" opacity="0.1" />
    <path d="M75,60 Q82,30 90,60 Q98,90 105,60 Q112,30 120,60" stroke="#C4B5FD" strokeWidth="1" fill="none" opacity="0.07" />
    <path d="M65,60 Q75,20 85,60 Q95,100 105,60 Q115,20 125,60 Q135,100 145,60" stroke="#DDD6FE" strokeWidth="0.8" fill="none" opacity="0.05" />
  </SceneWrap>
);

export const ObservationScene: React.FC = () => (
  <SceneWrap>
    <defs>
      <linearGradient id="cs-obs" x1="0" y1="0" x2="1" y2="0">
        <stop offset="0%" stopColor="#ECFDF5" />
        <stop offset="50%" stopColor="#FEF9C3" />
        <stop offset="100%" stopColor="#FDF2F8" />
      </linearGradient>
    </defs>
    <rect width="200" height="120" fill="url(#cs-obs)" opacity="0.15" />
    {/* 小径 */}
    <path d="M0,100 Q50,90 100,95 Q150,88 200,92" stroke="#D4A574" strokeWidth="3" fill="none" opacity="0.08" />
    {/* 长椅 */}
    <g opacity="0.1" transform="translate(80,78)">
      <rect x="0" y="8" width="20" height="2" rx="1" fill="#92400E" />
      <rect x="2" y="10" width="2" height="8" rx="0.5" fill="#92400E" />
      <rect x="16" y="10" width="2" height="8" rx="0.5" fill="#92400E" />
      <rect x="0" y="0" width="20" height="2" rx="1" fill="#92400E" />
    </g>
    {/* 小花 */}
    <circle cx="30" cy="100" r="2.5" fill="#FBCFE8" opacity="0.15" />
    <circle cx="140" cy="95" r="2" fill="#C4B5FD" opacity="0.12" />
    <circle cx="170" cy="98" r="3" fill="#FBCFE8" opacity="0.1" />
    <circle cx="55" cy="96" r="2" fill="#FDE047" opacity="0.12" />
  </SceneWrap>
);
