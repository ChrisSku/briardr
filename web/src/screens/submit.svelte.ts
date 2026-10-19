// Form submission state for sheets: busy while the request runs, and the error to show if it fails.
import { ApiError } from '../api.ts';

export class Submit {
  busy = $state(false);
  error = $state<string | null>(null);
  #fn: () => Promise<void>;
  constructor(fn: () => Promise<void>) {
    this.#fn = fn;
  }
  run = async (e?: Event) => {
    e?.preventDefault();
    this.busy = true;
    this.error = null;
    try {
      await this.#fn();
    } catch (err) {
      this.error = err instanceof ApiError ? err.message : 'Something went wrong.';
    } finally {
      this.busy = false;
    }
  };
}
