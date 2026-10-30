// Icon shapes and names, in plain TypeScript so code outside Svelte (tests, scripts) can import them.
import type { ClassValue } from 'svelte/elements';

export const SHAPES = {
  agents: '<path d="M12 3.5l1.9 4.6 4.6 1.9-4.6 1.9L12 16.5l-1.9-4.6L5.5 10l4.6-1.9z" /><path d="M18.5 15.5l.8 1.9 1.9.8-1.9.8-.8 1.9-.8-1.9-1.9-.8 1.9-.8z" />',
  arrows: '<path d="M12 3.5l2.5 3h-5zM12 20.5l-2.5-3h5zM3.5 12l3-2.5v5zM20.5 12l-3 2.5v-5z" fill="currentColor" stroke="none" /><circle cx="12" cy="12" r="1.6" />',
  back: '<path d="M15 5l-7 7 7 7" />',
  backspace: '<path d="M9 5.5h10a2 2 0 012 2v9a2 2 0 01-2 2H9L3 12z" /><path d="M11.5 9.5l5 5M16.5 9.5l-5 5" />',
  battery: '<rect x="2.5" y="7.5" width="17" height="9" rx="2.2" /><path d="M22 10.8v2.4" /><rect x="4.6" y="9.6" width="9.6" height="4.8" rx="1" fill="currentColor" stroke="none" />',
  bell: '<path d="M6 16.5V11a6 6 0 0112 0v5.5l1.5 2h-15z" /><path d="M10 20.5a2 2 0 004 0" />',
  bolt: '<path d="M13 2.5L5 13.5h6l-1 8 8-11h-6z" />',
  branch: '<circle cx="6.5" cy="5.5" r="2.2" /><circle cx="6.5" cy="18.5" r="2.2" /><circle cx="17.5" cy="7.5" r="2.2" /><path d="M6.5 7.7v8.6M17.5 9.7c0 4-4.5 4.3-9.4 7.2" />',
  changes: '<circle cx="6.5" cy="5.5" r="2.2" /><circle cx="6.5" cy="18.5" r="2.2" /><circle cx="17.5" cy="7.5" r="2.2" /><path d="M6.5 7.7v8.6M17.5 9.7c0 4-4.5 4.3-9.4 7.2" />',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5" />',
  chevron: '<path d="M9 6l6 6-6 6" />',
  'chevron-down': '<path d="M6 9l6 6 6-6" />',
  claude: '<path d="M12 3.5v17M3.5 12h17M6 6l12 12M18 6L6 18" stroke-width="2.2" />',
  clipboard: '<rect x="5.5" y="4.5" width="13" height="16" rx="2" /><path d="M9 4.5V3.5h6v1M9 10h6M9 13.5h6M9 17h3.5" />',
  close: '<path d="M6 6l12 12M18 6L6 18" />',
  codex: '<path d="M12 3l7.8 4.5v9L12 21l-7.8-4.5v-9z" /><path d="M9 10l2.5 2-2.5 2M13 14.5h3" />',
  collapse: '<path d="M8 3.5l4 4 4-4M8 20.5l4-4 4 4M5 12h14" />',
  copy: '<rect x="8.5" y="8.5" width="12" height="12" rx="2" /><path d="M15.5 8.5v-2a2 2 0 00-2-2h-7a2 2 0 00-2 2v7a2 2 0 002 2h2" />',
  device: '<rect x="7" y="2.5" width="10" height="19" rx="2" /><path d="M11 18.5h2" />',
  docs: '<path d="M6.5 3.5h7l4 4v13h-11z" /><path d="M13.5 3.5v4h4M9 12h6M9 15.5h6" />',
  dog: '<path d="M7 7.5c-2 0-3.5 2-3.5 5s1.2 4 2.3 4M17 7.5c2 0 3.5 2 3.5 5s-1.2 4-2.3 4" /><path d="M7 7.5c1.2-2 3-3 5-3s3.8 1 5 3c.8 1.5 1 3.5.7 6-.4 3.5-2.8 6-5.7 6s-5.3-2.5-5.7-6c-.3-2.5-.1-4.5.7-6z" /><path d="M10.6 14.6h2.8L12 16z" fill="currentColor" stroke="none" /><path d="M9.8 11h.01M14.2 11h.01" stroke-width="2.4" />',
  download: '<path d="M12 4v11M7 10.5l5 5 5-5M5 19.5h14" />',
  editor: '<path d="M8 6l-5 6 5 6M16 6l5 6-5 6M13.5 4.5l-3 15" />',
  enter: '<path d="M19 5v7a3 3 0 01-3 3H6" /><path d="M9.5 11.5L6 15l3.5 3.5" />',
  eye: '<path d="M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12z" /><circle cx="12" cy="12" r="3" />',
  file: '<path d="M6.5 3.5h7l4 4v13h-11z" /><path d="M13.5 3.5v4h4" />',
  folder: '<path d="M3.5 6.5a2 2 0 012-2h4l2 2h7a2 2 0 012 2v9a2 2 0 01-2 2h-13a2 2 0 01-2-2z" />',
  image: '<rect x="3" y="4.5" width="18" height="15" rx="2" /><circle cx="8.5" cy="9.5" r="1.7" /><path d="M21 16l-5.5-5.5L5 19.5" />',
  info: '<circle cx="12" cy="12" r="8.5" /><path d="M12 11v5.5M12 7.8h.01" />',
  keyboard: '<rect x="2.5" y="6" width="19" height="12" rx="2" /><path d="M6 9.5h.01M9 9.5h.01M12 9.5h.01M15 9.5h.01M18 9.5h.01M6 12.5h.01M18 12.5h.01M8 15h8" />',
  'keyboard-down': '<rect x="2.5" y="3.5" width="19" height="11" rx="2" /><path d="M6 7h.01M9 7h.01M12 7h.01M15 7h.01M18 7h.01M6 10h.01M18 10h.01M8 11.5h8M9.5 18l2.5 2.5 2.5-2.5" />',
  laptop: '<path d="M5 6.5A1.5 1.5 0 016.5 5h11A1.5 1.5 0 0119 6.5V15H5z" /><path d="M2.5 18.5h19" />',
  link: '<path d="M10 14a4 4 0 005.7 0l3-3a4 4 0 00-5.7-5.7l-1 1" /><path d="M14 10a4 4 0 00-5.7 0l-3 3a4 4 0 005.7 5.7l1-1" />',
  mic: '<rect x="9" y="3" width="6" height="11" rx="3" /><path d="M5.5 11a6.5 6.5 0 0013 0M12 17.5V21" />',
  minimize: '<path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5" />',
  minus: '<path d="M5 12h14" />',
  moon: '<path d="M20 14.5A8.5 8.5 0 019.5 4a8.5 8.5 0 1010.5 10.5z" />',
  more: '<circle cx="5.5" cy="12" r="1.3" fill="currentColor" stroke="none" /><circle cx="12" cy="12" r="1.3" fill="currentColor" stroke="none" /><circle cx="18.5" cy="12" r="1.3" fill="currentColor" stroke="none" />',
  notes: '<path d="M5 4.5h10.5l3.5 3.5v11.5H5z" /><path d="M8.5 10h7M8.5 13.5h7M8.5 17h4" />',
  offline: '<path d="M3 3l18 18M8.5 16.5a5 5 0 017 0M5 12.5a10 10 0 015-2.7M14.5 9.9A10 10 0 0119 12.5M2 9a15 15 0 014.4-2.7M12 4.5a15 15 0 0110 4.5" /><path d="M12 20h.01" />',
  pencil: '<path d="M14.5 5.5l4 4L9 19H5v-4z" /><path d="M12.5 7.5l4 4" />',
  plug: '<path d="M9 3v5M15 3v5M6.5 8h11v3a5.5 5.5 0 01-11 0zM12 16.5V21" />',
  plus: '<path d="M12 5v14M5 12h14" />',
  preview: '<circle cx="12" cy="12" r="8.5" /><path d="M3.5 12h17M12 3.5c2.5 2.6 2.5 14.4 0 17M12 3.5c-2.5 2.6-2.5 14.4 0 17" />',
  program: '<circle cx="12" cy="12" r="8.5" /><path d="M10 8.5l5 3.5-5 3.5z" fill="currentColor" stroke="none" />',
  refresh: '<path d="M20 11a8 8 0 10-2.3 5.7" /><path d="M20 5v6h-6" />',
  search: '<circle cx="11" cy="11" r="6.5" /><path d="M20 20l-4.2-4.2" />',
  settings: '<circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 00.3 1.8l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.8-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.8.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.8 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.8l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.8.3H9a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.8-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.8V9a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z" />',
  sidebar: '<rect x="3" y="4.5" width="18" height="15" rx="2.5" /><path d="M9.5 4.5v15M5.5 8.5h1.5M5.5 11.5h1.5" />',
  terminal: '<rect x="3" y="4.5" width="18" height="15" rx="2.5" /><path d="M7 9.5l3 2.5-3 2.5M12.5 15h4.5" />',
  trash: '<path d="M4.5 7h15M9.5 7V5h5v2M6.5 7l1 12.5h9l1-12.5" /><path d="M10.5 10.5v6M13.5 10.5v6" />',
  usage: '<path d="M4.5 17a8 8 0 1115 0" /><path d="M12 17l4-6" /><circle cx="12" cy="17" r="1.2" />',
  warning: '<path d="M12 4l9 16H3z" /><path d="M12 10v4.5M12 17.5h.01" />',
  wifi: '<path d="M2 9a15 15 0 0120 0M5 12.5a10 10 0 0114 0M8.5 16a5 5 0 017 0" /><path d="M12 20h.01" stroke-width="2.6" />',
  workspaces: '<path d="M3.5 6.5a2 2 0 012-2h4l2 2h7a2 2 0 012 2v9a2 2 0 01-2 2h-13a2 2 0 01-2-2z" /><path d="M7.5 12.5h9M7.5 15.5h5" />',
  wrap: '<path d="M4 6.5h16M4 12h12.5a3.5 3.5 0 010 7H12" /><path d="M14.5 16.5L12 19l2.5 2.5M4 19h4" />',
};
export type IconName = keyof typeof SHAPES;
/** A plugin's icon name, or a plug for names Briardr doesn't have. */
export function iconName(name: string | undefined): IconName {
  return name && Object.hasOwn(SHAPES, name) ? (name as IconName) : 'plug';
}
export interface IconProps {
  size?: number;
  label?: string;
  class?: ClassValue;
}
