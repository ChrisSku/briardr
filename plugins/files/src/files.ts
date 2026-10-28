// Files surface helpers: what the server sends, and which language a file is in (by name).
import { persisted } from '@briardr/sdk';

/** How files show on this device: line wrapping (null: on when compact) and the folder pane's width. */
export const filesView = persisted<{ wrap: boolean | null; treeWidth: number }>('briardr.files.view', { wrap: null, treeWidth: 280 });

export interface FileEntry {
  name: string;
  path: string;
  dir: boolean;
  size: number;
  link: boolean;
  changed: boolean;
}
export interface DirList {
  path: string;
  entries: FileEntry[];
  total: number;
  truncated: boolean;
  asOf: number;
}
export interface FileText {
  path: string;
  text: string | null;
  binary: boolean;
  size: number;
}
export interface FileSearch {
  entries: FileEntry[];
  truncated: boolean;
}

const BY_EXT: Record<string, string> = {
  sh: 'bash', bash: 'bash', zsh: 'bash', fish: 'bash', command: 'bash',
  c: 'c', h: 'c',
  cc: 'cpp', cpp: 'cpp', cxx: 'cpp', hpp: 'cpp', hh: 'cpp', hxx: 'cpp',
  cs: 'csharp',
  css: 'css', scss: 'scss', sass: 'scss', less: 'scss',
  dart: 'dart',
  diff: 'diff', patch: 'diff',
  ex: 'elixir', exs: 'elixir',
  go: 'go',
  graphql: 'graphql', gql: 'graphql',
  ini: 'ini', toml: 'ini', cfg: 'ini', conf: 'ini', properties: 'ini', env: 'ini',
  java: 'java',
  js: 'javascript', mjs: 'javascript', cjs: 'javascript', jsx: 'javascript',
  json: 'json', jsonc: 'json', json5: 'json', webmanifest: 'json',
  kt: 'kotlin', kts: 'kotlin',
  lua: 'lua',
  mk: 'makefile',
  md: 'markdown', markdown: 'markdown', mdx: 'markdown',
  m: 'objectivec', mm: 'objectivec',
  php: 'php',
  proto: 'protobuf',
  py: 'python', pyi: 'python',
  rb: 'ruby', rake: 'ruby', gemspec: 'ruby',
  rs: 'rust',
  sql: 'sql',
  swift: 'swift',
  ts: 'typescript', tsx: 'typescript', mts: 'typescript', cts: 'typescript',
  html: 'xml', htm: 'xml', xml: 'xml', svg: 'xml', plist: 'xml', svelte: 'xml', vue: 'xml', xhtml: 'xml',
  yml: 'yaml', yaml: 'yaml',
};
const BY_NAME: Record<string, string> = {
  dockerfile: 'dockerfile', containerfile: 'dockerfile', makefile: 'makefile', gnumakefile: 'makefile', gemfile: 'ruby', rakefile: 'ruby',
  '.bashrc': 'bash', '.zshrc': 'bash', '.profile': 'bash', '.envrc': 'bash', '.gitconfig': 'ini', '.editorconfig': 'ini', '.npmrc': 'ini',
  'cargo.lock': 'ini', 'bun.lock': 'json',
};
const LABELS: Record<string, string> = {
  bash: 'Shell', cpp: 'C++', csharp: 'C#', css: 'CSS', scss: 'SCSS', graphql: 'GraphQL', ini: 'Config', javascript: 'JavaScript', json: 'JSON',
  objectivec: 'Objective-C', php: 'PHP', protobuf: 'Protobuf', sql: 'SQL', typescript: 'TypeScript', xml: 'HTML/XML', yaml: 'YAML',
};

export const IMAGE_EXT = /\.(png|jpe?g|gif|webp|svg|avif)$/i;
export const PDF_EXT = /\.pdf$/i;

export function languageOf(path: string): string | null {
  const name = path.split('/').pop()!.toLowerCase();
  if (BY_NAME[name]) return BY_NAME[name];
  if (name.startsWith('dockerfile.')) return 'dockerfile';
  const dot = name.lastIndexOf('.');
  return dot > 0 || (dot === 0 && name.length > 1) ? BY_EXT[name.slice(dot + 1)] || null : null;
}

export const languageLabel = (lang: string | null) => (lang ? LABELS[lang] || lang[0].toUpperCase() + lang.slice(1) : 'Plain text');

export type FileIconKind = 'file' | 'badge' | 'code' | 'react' | 'vue' | 'svelte' | 'braces' | 'markdown' | 'text' | 'image' | 'config' | 'git' | 'lock' | 'package' | 'terminal' | 'database' | 'archive' | 'audio' | 'video' | 'font' | 'docker';
export interface FileIconSpec {
  kind: FileIconKind;
  tint: string;
  badge?: string;
}

const LANGUAGE_BADGES: Record<string, string> = {
  javascript: 'JS', typescript: 'TS', python: 'PY', rust: 'RS', swift: 'SW', go: 'GO',
  c: 'C', cpp: 'C++', csharp: 'C#', java: 'JV', kotlin: 'KT', ruby: 'RB', php: 'PHP',
  dart: 'D', elixir: 'EX', lua: 'LUA', objectivec: 'M', css: '#', scss: '#',
};

/** Filename conventions take priority over extensions (e.g. package.json and .env.local). */
export function fileIconOf(path: string): FileIconSpec {
  const name = path.split('/').pop()!.toLowerCase();
  const ext = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1) : '';
  const icon = (kind: FileIconKind, tint: string): FileIconSpec => ({ kind, tint });
  if (/^\.git/.test(name)) return icon('git', 'orange');
  if (name === '.env' || name.startsWith('.env.') || name === '.envrc') return icon('config', 'green');
  if (/\.(lock|lockb)$/.test(name) || /^(package-lock|npm-shrinkwrap)\.json$/.test(name) || name === 'pnpm-lock.yaml') return icon('lock', 'yellow');
  if (/^(package\.json|cargo\.toml|go\.mod|gemfile|pyproject\.toml|composer\.json|deno\.jsonc?)$/.test(name)) return icon('package', 'orange');
  if (/^(dockerfile|containerfile)(\.|$)/.test(name) || /^(docker-)?compose(\.[\w-]+)?\.ya?ml$/.test(name)) return icon('docker', 'blue');
  if (ext === 'jsx' || ext === 'tsx') return icon('react', 'blue');
  if (ext === 'svelte') return icon('svelte', 'orange');
  if (ext === 'vue') return icon('vue', 'green');
  if (/^(png|jpe?g|gif|webp|svg|avif|ico|bmp|tiff?|heic)$/.test(ext)) return icon('image', 'image');
  if (/^(zip|gz|tgz|bz2|xz|7z|rar|tar|zst)$/.test(ext)) return icon('archive', 'orange');
  if (/^(mp3|wav|ogg|m4a|aac|flac|aiff)$/.test(ext)) return icon('audio', 'purple');
  if (/^(mp4|mov|webm|mkv|avi|m4v)$/.test(ext)) return icon('video', 'purple');
  if (/^(woff2?|ttf|otf|eot)$/.test(ext)) return icon('font', 'image');
  if (/^(db|sqlite3?|sql)$/.test(ext)) return icon('database', 'blue');
  if (ext === 'pdf') return { kind: 'badge', tint: 'image', badge: 'PDF' };
  if (/^(txt|rtf|docx?|odt|log)$/.test(ext) || /^(licen[cs]e|copying|notice)(\.|$)/.test(name)) return icon('text', 'plain');
  if (/^(csv|tsv|xlsx?|ods)$/.test(ext)) return icon('database', 'green');
  const lang = languageOf(name);
  const tint = tintOf(name);
  if (lang && LANGUAGE_BADGES[lang]) return { kind: 'badge', tint, badge: LANGUAGE_BADGES[lang] };
  if (lang === 'markdown') return icon('markdown', 'teal');
  if (lang === 'json' || lang === 'graphql') return icon('braces', tint);
  if (lang === 'bash') return icon('terminal', 'green');
  if (lang === 'ini' || lang === 'yaml' || lang === 'makefile' || /(^|\.)config\./.test(name) || /^\.[\w-]+rc$/.test(name)) return icon('config', 'green');
  if (lang) return icon('code', tint);
  return icon('file', 'plain');
}

/** A theme-aware color family for source languages. */
function tintOf(path: string): string {
  const lang = languageOf(path);
  if (IMAGE_EXT.test(path)) return 'image';
  switch (lang) {
    case 'typescript':
      return 'blue';
    case 'javascript':
    case 'json':
      return 'yellow';
    case 'rust':
    case 'swift':
    case 'xml':
    case 'java':
      return 'orange';
    case 'python':
    case 'go':
    case 'css':
    case 'scss':
    case 'c':
    case 'cpp':
    case 'dart':
      return 'blue';
    case 'markdown':
      return 'teal';
    case 'bash':
    case 'yaml':
    case 'ini':
    case 'dockerfile':
    case 'makefile':
      return 'green';
    case 'ruby':
    case 'elixir':
    case 'kotlin':
    case 'php':
    case 'csharp':
      return 'purple';
    default:
      return 'plain';
  }
}

export function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
