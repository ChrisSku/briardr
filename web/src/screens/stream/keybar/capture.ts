/** Pointer capture keeps a hold or drag on its key; it can throw for pointers the browser no longer tracks. */
export function capture(e: PointerEvent) {
  try {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  } catch {}
}
