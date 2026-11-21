import React, { useContext, useEffect, useState } from 'react';

export const StreamFadeContext = React.createContext(false);
export const STREAM_FADE_MS = 420;

export interface FadeState {
  text: string;
  stable: string;
  chunks: { text: string; at: number; id: number }[];
  nextId: number;
}

/** Keep only the recent stream tail animated, without splitting Unicode characters. */
export function advanceFade(state: FadeState, text: string, now: number): FadeState {
  if (!text.startsWith(state.text)) {
    return { text, stable: '', chunks: text ? [{ text, at: now, id: state.nextId }] : [], nextId: state.nextId + 1 };
  }
  let stable = state.stable;
  const chunks = state.chunks.filter(chunk => {
    if (now - chunk.at < STREAM_FADE_MS) return true;
    stable += chunk.text;
    return false;
  });
  const added = text.slice(state.text.length);
  if (added) chunks.push({ text: added, at: now, id: state.nextId });
  return { text, stable, chunks, nextId: state.nextId + (added ? 1 : 0) };
}

const AnimatedText: React.FC<{ text: string }> = ({ text }) => {
  const [state, setState] = useState<FadeState>(() => advanceFade(
    { text: '', stable: '', chunks: [], nextId: 0 }, text, Date.now(),
  ));
  // Synchronize before paint so newly received text never flashes at full opacity.
  let current = state;
  if (state.text !== text) {
    current = advanceFade(state, text, Date.now());
    setState(current);
  }
  useEffect(() => {
    if (!current.chunks.length) return;
    const timer = window.setTimeout(() => {
      setState(previous => advanceFade(previous, previous.text, Date.now()));
    }, Math.max(1, current.chunks[0].at + STREAM_FADE_MS - Date.now()));
    return () => window.clearTimeout(timer);
  }, [current]);
  return <>{current.stable}{current.chunks.map(chunk => (
    <span key={chunk.id} className="codex-stream-fade">{chunk.text}</span>
  ))}</>;
};

export const StreamFadeText: React.FC<{ text: string }> = ({ text }) => (
  useContext(StreamFadeContext) ? <AnimatedText text={text} /> : <>{text}</>
);
