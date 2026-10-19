// Work that can wait until the browser has nothing else to do (Safari has no requestIdleCallback).
export function whenIdle(fn: () => void, timeout = 3000) {
  const idle = (globalThis as { requestIdleCallback?: (fn: () => void, o?: { timeout: number }) => void }).requestIdleCallback;
  if (idle) idle(fn, { timeout });
  else setTimeout(fn, 1500);
}
