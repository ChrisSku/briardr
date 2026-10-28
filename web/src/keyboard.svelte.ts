// The on-screen keyboard's height (0 when it's hidden), from the visual viewport. One tracker for the
// app: it also sets --kb on the root, so layouts keep fields and buttons above the keyboard (sheets end
// on top of it, scrolling screens get room to scroll), and it keeps the focused field in view.

let kb = $state(0);
let started = false;

// The layout viewport's height. Not innerHeight: once iOS has scrolled the page to reveal a field low on
// the screen (a sheet's), a Home Screen app's innerHeight shrinks with the keyboard, and stays so.
const layoutHeight = () => document.documentElement.clientHeight;

/** How far the visual viewport ends above the bottom of the layout: the on-screen keyboard, or about 0. */
export function viewportInset(): number {
  const vv = window.visualViewport;
  return vv ? Math.max(0, Math.round(layoutHeight() - vv.height - vv.offsetTop)) : 0;
}

/** Starts tracking the keyboard; the app does this once at startup. */
export function trackKeyboard() {
  const vv = window.visualViewport;
  if (started || !vv) return;
  started = true;
  const update = () => {
    const inset = viewportInset();
    const next = inset > 60 ? inset : 0;
    if (next !== kb) {
      kb = next;
      document.documentElement.style.setProperty('--kb', `${next}px`);
    }
    // iOS may pan the page to reveal the focused input; the layout is fixed, so undo the pan.
    if (vv.offsetTop > 0 && document.scrollingElement) document.scrollingElement.scrollTop = 0;
    reveal();
  };
  vv.addEventListener('resize', update);
  vv.addEventListener('scroll', update);
  // Moving to the next field while the keyboard stays up.
  document.addEventListener('focusin', () => kb && requestAnimationFrame(reveal));
  update();
}

// With the pan undone, a focused field low on a scrolling screen can sit under the keyboard: scroll
// its container until the field is above it. The terminal places itself.
function reveal() {
  const el = document.activeElement;
  if (!kb || !(el instanceof HTMLElement) || !el.matches('input, textarea, [contenteditable]') || el.closest('.term-surface')) return;
  const r = el.getBoundingClientRect();
  const below = r.bottom - (layoutHeight() - kb - 8);
  if (below <= 0) return;
  for (let p = el.parentElement; p; p = p.parentElement) {
    const { overflowY } = getComputedStyle(p);
    if ((overflowY === 'auto' || overflowY === 'scroll') && p.scrollHeight > p.clientHeight) {
      // A field taller than the space left keeps its top in view.
      p.scrollTop += Math.min(below, Math.max(0, r.top - p.getBoundingClientRect().top - 8));
      return;
    }
  }
}

export function keyboardInset(): { readonly current: number } {
  trackKeyboard();
  return {
    get current() {
      return kb;
    },
  };
}
