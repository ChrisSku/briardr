// Draws the Mermaid diagrams renderMarkdown leaves as code blocks. Mermaid is big, so it loads only when a
// document has a diagram; until then, and if a diagram can't be drawn, the code block stays. A document
// can't change this configuration (`secure`), and its diagrams keep no links.
import type { Mermaid } from 'mermaid';

let mermaid: Promise<Mermaid> | null = null;
let drawn = 0;
const MIN_SCALE = 0.7;

function configure(m: Mermaid) {
  const dark = matchMedia('(prefers-color-scheme: dark)').matches;
  const styles = getComputedStyle(document.documentElement);
  const color = (name: string) => styles.getPropertyValue(name).trim();
  const fontFamily = color('--font-sans');
  m.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    suppressErrorRendering: true,
    htmlLabels: false,
    secure: ['secure', 'securityLevel', 'startOnLoad', 'maxTextSize', 'maxEdges', 'suppressErrorRendering', 'htmlLabels', 'theme', 'themeVariables', 'themeCSS', 'fontFamily', 'altFontFamily'],
    theme: 'base',
    fontFamily,
    themeVariables: {
      darkMode: dark,
      fontFamily,
      background: color('--bg-canvas'),
      primaryColor: color('--bg-sunken'),
      primaryTextColor: color('--text-primary'),
      primaryBorderColor: color('--line-strong'),
      lineColor: color('--text-secondary'),
      textColor: color('--text-primary'),
      edgeLabelBackground: color('--bg-canvas'),
    },
  });
}

async function draw(m: Mermaid, block: HTMLElement) {
  try {
    const { svg } = await m.render(`md-diagram-${++drawn}`, block.textContent || '');
    if (!block.isConnected) return;
    const view = document.createElement('figure');
    view.className = 'md-diagram-view';
    view.tabIndex = 0;
    view.setAttribute('role', 'region');
    view.setAttribute('aria-label', 'Diagram');
    view.innerHTML = svg;
    // A wide diagram scrolls, like a table, rather than shrinking until a phone can't show its text.
    const drawing = view.querySelector('svg');
    if (drawing) drawing.style.minWidth = `${Math.round(drawing.viewBox.baseVal.width * MIN_SCALE)}px`;
    for (const a of view.querySelectorAll('a')) a.replaceWith(...a.childNodes);
    if (block.nextElementSibling?.matches('.md-diagram-view')) block.nextElementSibling.remove();
    block.after(view);
    block.hidden = true;
  } catch {
    // Not a diagram Mermaid can draw: it stays readable as code.
  }
}

function load(): Promise<Mermaid> {
  return (mermaid ??= import('mermaid').then(({ default: m }) => {
    // Diagrams take their colours when drawn, so a change of theme draws them again.
    matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
      configure(m);
      for (const block of document.querySelectorAll<HTMLElement>('pre.md-diagram')) void draw(m, block);
    });
    configure(m);
    return m;
  }));
}

/** Draws the diagrams in an element holding renderMarkdown's output. Call it after each render. */
export async function renderDiagrams(root: ParentNode): Promise<void> {
  const blocks = [...root.querySelectorAll<HTMLElement>('pre.md-diagram')];
  if (!blocks.length) return;
  try {
    const m = await load();
    for (const block of blocks) await draw(m, block);
  } catch {
    mermaid = null; // The chunk didn't load (offline): the code blocks stay, and the next render tries again.
  }
}
