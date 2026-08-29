import React, { useState, useCallback } from 'react';

interface FlipCardProps {
  front: React.ReactNode;
  back: React.ReactNode;
  style?: React.CSSProperties;
}

const FlipCard: React.FC<FlipCardProps> = React.memo(({ front, back, style }) => {
  const [flipped, setFlipped] = useState(false);

  const toggle = useCallback(() => setFlipped((v) => !v), []);

  return (
    <div
      style={{
        position: 'relative',
        perspective: '800px',
        cursor: 'pointer',
        ...style,
      }}
      onClick={toggle}
    >
      <div
        style={{
          position: 'relative',
          width: '100%',
          height: '100%',
          transition: 'transform 350ms cubic-bezier(0.4, 0, 0.2, 1)',
          transformStyle: 'preserve-3d',
          transform: flipped ? 'rotateY(180deg)' : 'rotateY(0deg)',
        }}
      >
        <div
          style={{
            position: 'absolute',
            inset: 0,
            backfaceVisibility: 'hidden',
            WebkitBackfaceVisibility: 'hidden',
          }}
        >
          {front}
        </div>
        <div
          style={{
            position: 'absolute',
            inset: 0,
            backfaceVisibility: 'hidden',
            WebkitBackfaceVisibility: 'hidden',
            transform: 'rotateY(180deg)',
          }}
        >
          {back}
        </div>
      </div>
    </div>
  );
});

FlipCard.displayName = 'FlipCard';
export default FlipCard;
