// Deterministic fixture data for the frontend benchmark. The same seed and size always produce the same
// workspaces, streams, changes, docs, notes and terminal screens, so both implementations see
// byte-identical API and WebSocket responses.
import type { AgentState, Model, Project, Stream, StreamKind, Workspace } from '../web/src/types.ts';

/** Small seeded PRNG (mulberry32), so fixtures never depend on Math.random. */
export function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const WORDS = ['alpha', 'bravo', 'charlie', 'delta', 'echo', 'foxtrot', 'golf', 'hotel', 'india', 'juliet', 'kilo', 'lima', 'mike', 'november', 'oscar', 'papa', 'quebec', 'romeo', 'sierra', 'tango', 'uniform', 'victor', 'whiskey', 'xray', 'yankee', 'zulu'];
const TASKS = ['refactor auth', 'fix flaky test', 'add retries', 'update docs', 'migrate schema', 'tune queries', 'review PR', 'bump deps', 'write e2e', 'split module', 'profile build', 'clean logs'];

export interface Fixture {
  size: number;
  model: Model;
  /** Stream IDs picked for the scenarios (stable per size). */
  picks: { terminal: string; live: string; burst: string; searchTerm: string; searchMatches: number };
  counts: Record<'attention' | 'working' | 'finished' | 'ready' | 'unknown', number>;
}

/** `size` streams in workspaces of 2–4 projects with 5 streams each. Times are relative to `now`. */
export function makeFixture(size: number, now: number, seed = 7): Fixture {
  const r = rng(seed + size);
  const perProject = 5;
  const projectsPerWs = size >= 300 ? 4 : 2;
  const nProjects = Math.ceil(size / perProject);
  const nWs = Math.ceil(nProjects / projectsPerWs);
  const workspaces: Workspace[] = [];
  const projects: Project[] = [];
  const streams: Stream[] = [];
  for (let w = 0; w < nWs; w++) {
    const id = `w${w + 1}`;
    const name = `${WORDS[w % WORDS.length]}${w >= WORDS.length ? '-' + Math.floor(w / WORDS.length) : ''}`;
    workspaces.push({ id, name, number: w + 1, focused: w === 0, activeProjectId: `${id}:p1`, cwd: `/Users/dev/code/${name}` });
  }
  for (let p = 0; p < nProjects; p++) {
    const ws = workspaces[Math.floor(p / projectsPerWs)];
    const n = (p % projectsPerWs) + 1;
    projects.push({ id: `${ws.id}:p${n}`, workspaceId: ws.id, name: ['api', 'web', 'infra', 'docs'][n - 1], number: n, focused: false });
  }
  const kinds: StreamKind[] = [];
  for (let i = 0; i < size; i++) {
    const x = r();
    kinds.push(x < 0.6 ? 'agent' : x < 0.85 ? 'shell' : x < 0.95 ? 'editor' : 'program');
  }
  for (let i = 0; i < size; i++) {
    const pr = projects[Math.floor(i / perProject)];
    const ws = workspaces.find((w) => w.id === pr.workspaceId)!;
    const kind = kinds[i];
    let state: AgentState | null = null;
    if (kind === 'agent') {
      const x = r();
      state = x < 0.1 ? 'blocked' : x < 0.4 ? 'working' : x < 0.6 ? 'done' : x < 0.9 ? 'idle' : 'unknown';
    }
    const tool = kind === 'agent' ? (r() < 0.65 ? 'claude' : 'codex') : kind === 'shell' ? 'zsh' : kind === 'editor' ? 'nvim' : 'node';
    const task = TASKS[Math.floor(r() * TASKS.length)];
    const label = kind === 'agent' ? `${task} ${i + 1}` : null;
    const folder = `${ws.name}/${pr.name}`;
    streams.push({
      id: `s${i + 1}`,
      workspaceId: ws.id,
      projectId: pr.id,
      terminalId: `t${i + 1}`,
      name: label || `${tool} ${i + 1}`,
      label,
      kind,
      tool,
      state,
      stateChangeSeq: state ? 1 + Math.floor(r() * 40) : null,
      stateSince: state ? now - Math.floor(r() * 3 * 3600_000) : null,
      cwd: `/Users/dev/code/${folder}`,
      folder,
      focused: i === 0,
      title: null,
      rows: 40,
    });
  }
  const counts = { attention: 0, working: 0, finished: 0, ready: 0, unknown: 0 };
  for (const s of streams)
    if (s.kind === 'agent') counts[s.state === 'blocked' ? 'attention' : s.state === 'working' ? 'working' : s.state === 'done' ? 'finished' : s.state === 'idle' ? 'ready' : 'unknown']++;
  const agents = streams.filter((s) => s.kind === 'agent');
  const term = agents.find((s) => s.state === 'working')!;
  const live = agents.filter((s) => s.state === 'working')[1]!;
  const burst = agents.filter((s) => s.state === 'working')[2]!;
  const searchTerm = 'flaky';
  const searchMatches = streams.filter((s) => s.name.includes(searchTerm)).length;
  return {
    size,
    model: { workspaces, projects, streams, focusedStreamId: streams[0].id, herdrVersion: '0.9.3', asOf: now },
    picks: { terminal: term.id, live: live.id, burst: burst.id, searchTerm, searchMatches },
    counts,
  };
}

export const mac = (now: number) => ({
  name: 'Bench Mac',
  battery: { present: true, percent: 76, source: 'battery', charging: false, charged: false, minutes: 312, timeKind: 'remaining' },
  keepAwake: { briardr: true, others: [], sleepDisabled: false },
  asOf: now,
});

export const herdrHealth = (now: number) => ({ ok: true, problem: null, version: '0.9.3', lastOkAt: now });

export const settings = {
  herdrPath: '/opt/homebrew/bin/herdr',
  herdrSession: 'bench',
  herdr: { running: true, version: '0.9.3', problem: null },
  notifications: { includeNames: true, needsYou: true, finished: true, ready: false, lowBattery: true },
  usage: { claudeKeychainOptIn: true, codexEnabled: true },
  power: { keepAwake: true },
  tls: { wanted: 'local', effective: 'local' },
  caProfileAvailable: true,
  dictation: false,
  version: '0.1.0',
};

export function usage(now: number) {
  const win = (id: string, label: string, used: number, resetIn: number) => ({ id, label, usedPercent: used, resetsAt: now + resetIn });
  const status = (report: object) => ({ report, tapReport: null, tapInstalled: false, lastError: null, lastAttemptAt: now - 60_000, cooldownUntil: null, rateLimitedUntil: null, failures: 0, enabled: true });
  return {
    claude: status({ provider: 'claude', source: 'oauth', plan: 'Max', windows: [win('5h', '5-hour', 42, 2 * 3600_000), win('7d', 'Weekly', 71, 3 * 86400_000), win('7d_opus', 'Weekly (Opus)', 93, 3 * 86400_000)], capturedAt: now - 60_000 }),
    codex: status({ provider: 'codex', source: 'app-server', plan: 'Pro', windows: [win('primary', '5-hour', 12, 4 * 3600_000), win('secondary', 'Weekly', 35, 5 * 86400_000)], credits: { hasCredits: true, unlimited: false, balance: '120' }, resetCredits: { available: 1, credits: [{ title: 'Limit reset', description: null, grantedAt: now - 86400_000, expiresAt: now + 6 * 86400_000 }] }, capturedAt: now - 90_000 }),
  };
}

export const devices = (now: number) => ({
  devices: [
    { id: 'd1', name: 'Bench browser', createdAt: now - 10 * 86400_000, lastSeenAt: now, current: true, push: false },
    { id: 'd2', name: 'iPad', createdAt: now - 30 * 86400_000, lastSeenAt: now - 3600_000, current: false, push: true },
  ],
});

export function changes(now: number) {
  const r = rng(99);
  const files = Array.from({ length: 40 }, (_, i) => {
    const change = (['modified', 'modified', 'added', 'deleted', 'renamed', 'untracked'] as const)[i % 6];
    return {
      path: `src/${['server', 'web', 'lib', 'test'][i % 4]}/module-${i + 1}.ts`,
      origPath: change === 'renamed' ? `src/old/module-${i + 1}.ts` : null,
      index: '.',
      worktree: 'M',
      staged: i % 5 === 0,
      unstaged: true,
      untracked: change === 'untracked',
      conflicted: false,
      change,
      modeChange: null,
      binary: i === 7,
      additions: i === 7 ? null : 1 + Math.floor(r() * 120),
      deletions: i === 7 ? null : Math.floor(r() * 60),
      signature: `sig-${i + 1}`,
    };
  });
  return { state: 'ok', root: '/Users/dev/code/alpha/api', branch: 'feature/bench', detached: false, headShort: 'abc1234', noCommits: false, files, totalFiles: files.length, truncated: false, asOf: now };
}

export function diff(path: string) {
  const lines = [`diff --git a/${path} b/${path}`, 'index 1111111..2222222 100644', `--- a/${path}`, `+++ b/${path}`];
  for (let h = 0; h < 6; h++) {
    lines.push(`@@ -${h * 40 + 1},12 +${h * 40 + 1},14 @@ export function part${h}() {`);
    for (let i = 0; i < 12; i++) lines.push(i % 4 === 0 ? `-  const old${i} = compute(${i});` : i % 4 === 1 ? `+  const next${i} = compute(${i}, options);` : `   return value${i} + ${i};`);
  }
  return { text: lines.join('\n') + '\n', truncated: false, lines: lines.length, binary: false };
}

export function docsList(now: number) {
  const docs = Array.from({ length: 20 }, (_, i) => ({ path: i === 0 ? 'README.md' : `docs/guide-${i}.md`, name: i === 0 ? 'README.md' : `guide-${i}.md`, changed: i % 7 === 3, readme: i === 0, size: 4000 + i * 100 }));
  return { docs, total: docs.length, truncated: false, folder: 'alpha/api', asOf: now };
}

export function docText(path: string): string {
  const parts = [`# ${path}`, '', 'Briardr benchmark document with **bold**, _emphasis_, `code` and [a link](docs/guide-2.md).', ''];
  for (let s = 1; s <= 8; s++) {
    parts.push(`## Section ${s}`, '', `Paragraph ${s} explains the design. It links to [section ${s % 8 + 1}](#section-${s % 8 + 1}) and to https://example.com/page/${s}.`, '');
    parts.push('- [x] done item', '- [ ] open item', '- plain item with `inline code`', '');
    parts.push('| Column A | Column B | Column C |', '|---|---|---|', ...Array.from({ length: 5 }, (_, i) => `| a${i} | b${i} | c${i} |`), '');
    parts.push('```ts', `export function section${s}(x: number) {`, '  return x * 2;', '}', '```', '');
  }
  return parts.join('\n');
}

export function noteText(i: number): string {
  return `## Note ${i}\n\nNext prompts for the agent:\n\n- ask Claude to add retries to the ${WORDS[i % WORDS.length]} client\n- check the flaky test in \`test/${i}.ts\`\n- [ ] review the migration plan\n\n| step | owner |\n|---|---|\n| one | me |\n| two | agent |\n\n${'More context. '.repeat(30)}`;
}

/** A full-screen terminal redraw: `rows` lines of coloured text, marked with the frame number. */
export function screen(cols: number, rows: number, frame: number): string {
  let out = '\x1b[H\x1b[2J';
  for (let r = 0; r < rows; r++) {
    const head = r === 0 ? `FRAME ${frame} ` : `${String(r).padStart(3, ' ')} `;
    let line = `\x1b[3${(r + frame) % 7 + 1}m${head}\x1b[0m`;
    let len = head.length;
    let w = 0;
    while (len < cols - 10) {
      const word = WORDS[(r * 7 + w + frame) % WORDS.length];
      line += (w % 3 === 0 ? `\x1b[1m${word}\x1b[0m ` : `${word} `);
      len += word.length + 1;
      w++;
    }
    out += line + (r < rows - 1 ? '\r\n' : '');
  }
  return out;
}

export function encodeFrame(seq: number, width: number, height: number, text: string): Uint8Array {
  const body = new TextEncoder().encode(text);
  const buf = new Uint8Array(10 + body.length);
  const dv = new DataView(buf.buffer);
  buf[0] = 1;
  buf[1] = 1;
  dv.setUint32(2, seq >>> 0);
  dv.setUint16(6, width);
  dv.setUint16(8, height);
  buf.set(body, 10);
  return buf;
}
