// App-wide confirmation sheet and action menu. Any module can open them; App renders the hosts.
import type { Snippet } from 'svelte';
import { state } from '../reactive.svelte.ts';
import type { IconName } from './icons/index.ts';

// ---------- confirm ----------

export interface ConfirmReq {
  title: string;
  /** Plain text, or a snippet for richer content. */
  body: string | Snippet;
  confirm: string;
  destructive?: boolean;
  resolve: (ok: boolean) => void;
}
export const confirmReq = state<ConfirmReq | null>(null);

export function confirmDialog(title: string, body: string | Snippet, confirm: string, destructive = false): Promise<boolean> {
  return new Promise((resolve) => (confirmReq.value = { title, body, confirm, destructive, resolve }));
}

// ---------- menus ----------

export interface MenuItem {
  label: string;
  icon?: IconName;
  onSelect: () => void;
  disabled?: boolean;
  destructive?: boolean;
  hint?: string;
}

export interface MenuReq {
  items: MenuItem[];
  x: number;
  y: number;
  title?: string;
}
export const menuReq = state<MenuReq | null>(null);

export function openMenu(items: MenuItem[], at: { x: number; y: number } | HTMLElement, title?: string) {
  let x: number;
  let y: number;
  if (at instanceof HTMLElement) {
    const r = at.getBoundingClientRect();
    x = r.right;
    y = r.bottom + 4;
  } else ({ x, y } = at);
  menuReq.value = { items, x, y, title };
}
