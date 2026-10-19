// Pull to refresh for a scrolling surface: pull down past 64 px at the top and let go.
export class PullToRefresh {
  pull = $state(0);
  #start: number | null = null;
  #onRefresh: () => void;
  constructor(onRefresh: () => void) {
    this.#onRefresh = onRefresh;
  }
  handlers = {
    ontouchstart: (e: TouchEvent) => {
      const el = e.currentTarget as HTMLElement;
      this.#start = el.scrollTop <= 0 ? e.touches[0].clientY : null;
    },
    ontouchmove: (e: TouchEvent) => {
      if (this.#start === null) return;
      this.pull = Math.max(0, Math.min(90, e.touches[0].clientY - this.#start));
    },
    ontouchend: () => {
      if (this.pull > 64) this.#onRefresh();
      this.#start = null;
      this.pull = 0;
    },
  };
}
