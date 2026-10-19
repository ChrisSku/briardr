// Installing Briardr as an app: the browser's own install prompt where there is one (Chromium), otherwise
// the steps for this browser. Imported first thing by main.ts, because the prompt's event fires early.
import { record } from './diag.ts';
import { persisted, state } from './reactive.svelte.ts';

interface BeforeInstallPromptEvent extends Event {
  prompt(): Promise<void>;
  userChoice: Promise<{ outcome: 'accepted' | 'dismissed' }>;
}

/** Open as an installed app (Home Screen, Dock, app window), not in a browser tab. */
export const standalone = matchMedia('(display-mode: standalone)').matches || (navigator as { standalone?: boolean }).standalone === true;
/** The browser's install prompt, while it offers one. */
export const installPrompt = state<BeforeInstallPromptEvent | null>(null);
/** Installed from this tab just now (the app opens separately). */
export const justInstalled = state(false);
/**
 * The suggestion to install, shown on the lists after pairing in a browser tab: 'pending' until it's
 * dismissed or Briardr is installed. Never shown in the installed app, or before pairing.
 */
export const installSuggestion = persisted<'pending' | 'dismissed' | null>('briardr.install.suggest', null);

addEventListener('beforeinstallprompt', (e) => {
  e.preventDefault(); // offered from Settings or the suggestion instead of the browser's own banner
  installPrompt.value = e as BeforeInstallPromptEvent;
});
addEventListener('appinstalled', () => {
  installPrompt.value = null;
  justInstalled.value = true;
  if (installSuggestion.value === 'pending') installSuggestion.value = 'dismissed';
  record('app_installed', {});
});
if (standalone && installSuggestion.value === 'pending') installSuggestion.value = 'dismissed';

/** Shows the browser's install prompt. */
export async function promptInstall(): Promise<void> {
  const e = installPrompt.value;
  if (!e) return;
  installPrompt.value = null; // a prompt can be shown once
  await e.prompt();
  const { outcome } = await e.userChoice;
  record('install_prompt', { reason: outcome });
}

export type Platform = 'iphone' | 'ipad' | 'mac-safari' | 'android' | 'firefox' | 'desktop';

export function platform(): Platform {
  const ua = navigator.userAgent;
  if (/iPad/.test(ua) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1)) return 'ipad';
  if (/iPhone|iPod/.test(ua)) return 'iphone';
  if (/Android/.test(ua)) return 'android';
  if (/Firefox\//.test(ua)) return 'firefox';
  if (/Macintosh/.test(ua) && /Safari\//.test(ua) && !/Chrome|Chromium|Edg\//.test(ua)) return 'mac-safari';
  return 'desktop';
}

/** How to install in this browser, for when it has no install prompt to offer. */
export function installSteps(p: Platform = platform()): string {
  switch (p) {
    case 'iphone':
    case 'ipad':
      return 'In Safari, tap Share (it may be in the ⋯ menu), then “Add to Home Screen”. Keep “Open as Web App” on if it’s offered.';
    case 'mac-safari':
      return 'In Safari, choose File → Add to Dock.';
    case 'android':
      return 'Open the browser menu (⋮) and choose “Install app” or “Add to Home screen”.';
    case 'firefox':
      return 'Firefox can’t install Briardr as an app on this computer. Open this page in Safari, Chrome or Edge to install it, or keep using it in a tab.';
    default:
      return 'Use the install button in the address bar, or “Install” in the browser’s menu (in Chrome: Cast, save and share → Install page as app).';
  }
}

/** Where the installed app is, once it's installed. */
export function launchHint(p: Platform = platform()): string {
  switch (p) {
    case 'iphone':
    case 'ipad':
    case 'android':
      return 'Open Briardr from its Home Screen icon.';
    case 'mac-safari':
      return 'Open Briardr from the Dock or the Applications folder.';
    default:
      return 'Open Briardr from your apps (Launchpad, the Start menu or the app launcher), or from the window the browser opens.';
  }
}
