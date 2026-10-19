// The on-screen keyboard's height (0 when it's hidden), from the visual viewport. One tracker for the
// app: it also sets --kb on the root, so layouts keep fields and buttons above the keyboard (sheets end
// on top of it, scrolling screens get room to scroll), and it keeps the focused field in view.

let kb = $state(0);
let started = false;

/** Starts tracking the keyboard; the app does this once at startup. */
export function trackKeyboard() {
  const vv = window.visualViewport;
  if (started || !vv) return;
  started = true;
  const update = () => {
    const inset = Math.max(0, Math.round(window.innerHeight - vv.height - vv.offsetTop));
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
  const below = r.bottom - (window.innerHeight - kb - 8);
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
