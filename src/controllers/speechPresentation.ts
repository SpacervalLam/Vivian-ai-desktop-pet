/** Each queued sentence owns its expression; later metadata cannot rewrite it. */
export interface SpeechPresentation {
  expression?: string;
  expression_duration_ms?: number;
  motion?: string;
  gaze?: string;
  bubble: boolean;
  typing_indicator: boolean;
}

export interface SpeechSegment {
  text: string;
  characterId?: string;
  streamId?: string;
  presentation: SpeechPresentation | null;
}

export function mergePresentation(previous: SpeechPresentation | null, meta: { expression?: string; motion?: string; expressionDurationMs?: number }): SpeechPresentation {
  return {
    ...previous,
    ...(meta.expression ? { expression: meta.expression, expression_duration_ms: meta.expressionDurationMs } : {}),
    ...(meta.motion ? { motion: meta.motion } : {}),
    bubble: false,
    typing_indicator: false,
  };
}

export function speechSegment(text: string, presentation: SpeechPresentation | null, characterId?: string, streamId?: string): SpeechSegment {
  return { text, characterId, streamId, presentation: presentation ? { ...presentation } : null };
}

export function compactSpeechSegments(queue: SpeechSegment[]): SpeechSegment[] {
  const result: SpeechSegment[] = [];
  for (const segment of queue) {
    const previous = result[result.length - 1];
    if (previous && previous.characterId === segment.characterId && previous.streamId === segment.streamId
      && JSON.stringify(previous.presentation) === JSON.stringify(segment.presentation)) {
      previous.text += segment.text;
    } else result.push({ ...segment });
  }
  return result;
}
