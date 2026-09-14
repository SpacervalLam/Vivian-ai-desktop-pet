import React from 'react';

export interface LoadingSpinnerProps {
  size?: number;
  color?: string;
  thickness?: number;
}

const LoadingSpinner: React.FC<LoadingSpinnerProps> = ({
  size = 32,
  color = 'var(--accent)',
  thickness = 2,
}) => {
  return (
    <div
      style={{
        width: size,
        height: size,
        display: 'inline-block',
        position: 'relative',
        flexShrink: 0,
      }}
      role="status"
      aria-label="loading"
    >
      <svg
        width={size}
        height={size}
        viewBox={`0 0 ${size} ${size}`}
        style={{ display: 'block', animation: 'vivian-spin 0.9s linear infinite' }}
      >
        <defs>
          <linearGradient id="vivian-spinner-gradient" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor={color} stopOpacity="1" />
            <stop offset="60%" stopColor={color} stopOpacity="0.8" />
            <stop offset="100%" stopColor={color} stopOpacity="0" />
          </linearGradient>
        </defs>
        <circle
          cx={size / 2}
          cy={size / 2}
          r={size / 2 - thickness}
          fill="none"
          stroke={color}
          strokeOpacity="0.18"
          strokeWidth={thickness}
        />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={size / 2 - thickness}
          fill="none"
          stroke="url(#vivian-spinner-gradient)"
          strokeWidth={thickness}
          strokeLinecap="round"
          strokeDasharray={`${(size / 2 - thickness) * 2 * 0.75} ${(size / 2 - thickness) * 2}`}
        />
      </svg>
      <style>{`
        @keyframes vivian-spin {
          from { transform: rotate(0deg); }
          to { transform: rotate(360deg); }
        }
      `}</style>
    </div>
  );
};

export default LoadingSpinner;
