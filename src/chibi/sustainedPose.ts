interface SustainedPoseEnv {
  enter: () => Promise<boolean>;
  leave: () => Promise<boolean>;
  interrupted: () => boolean;
  now: () => number;
  wait: (ms: number) => Promise<void>;
  onHold?: () => void;
  onLeave?: () => void;
}

/** An entrance, a real held state, and an exit. A takeover never plays a stale exit. */
export async function runSustainedPose(holdMs: number, env: SustainedPoseEnv): Promise<boolean> {
  if (env.interrupted() || !await env.enter() || env.interrupted()) return false;
  env.onHold?.();
  const until = env.now() + holdMs;
  while (env.now() < until) {
    if (env.interrupted()) return false;
    await env.wait(Math.min(100, until - env.now()));
  }
  if (env.interrupted()) return false;
  env.onLeave?.();
  return await env.leave() && !env.interrupted();
}
