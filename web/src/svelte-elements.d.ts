// Safari's autocorrect attribute on text areas (Svelte's element types only list it for inputs).
import 'svelte/elements';

declare module 'svelte/elements' {
  interface HTMLTextareaAttributes {
    autocorrect?: 'on' | 'off' | null;
  }
}
