// The on-screen keyboard's height (0 when it's hidden), from the visual viewport.

export function keyboardInset(): { readonly current: number } {
  let kb = $state(0);
  $effect(() => {
    const vv = window.visualViewport;
    if (!vv) return;
    const update = () => {
      const inset = Math.max(0, Math.round(window.innerHeight - vv.height - vv.offsetTop));
      kb = inset > 60 ? inset : 0;
      // iOS may pan the page to reveal the focused input; the layout is fixed, so undo the pan.
      if (vv.offsetTop > 0 && document.scrollingElement) document.scrollingElement.scrollTop = 0;
    };
    vv.addEventListener('resize', update);
    vv.addEventListener('scroll', update);
    update();
    return () => {
      vv.removeEventListener('resize', update);
      vv.removeEventListener('scroll', update);
    };
  });
  return {
    get current() {
      return kb;
    },
  };
}
