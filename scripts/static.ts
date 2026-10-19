// The web client's static files: content types, cache headers, and the compressed copies the build
// writes next to each text file (file.br, file.gz; scripts/build.ts), served to browsers that accept
// them. A TypeScript mirror of the server's server/src/static_files.rs, for the build and for the
// benchmark's fixture server (bench/server.ts), which serves files the same way. Keep the two in step.
import fs from 'node:fs';
import path from 'node:path';

export const MIME: Record<string, string> = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.json': 'application/json',
  '.webmanifest': 'application/manifest+json', '.png': 'image/png', '.svg': 'image/svg+xml', '.ico': 'image/x-icon', '.woff2': 'font/woff2', '.map': 'application/json',
};

/** File types the build writes compressed copies of. Images and fonts are compressed already. */
export const COMPRESSED_TYPES = new Set(['.html', '.js', '.css', '.json', '.webmanifest', '.svg']);

/** Content encodings, best first, with the suffix of the copy the build writes for each. */
const ENCODINGS = [
  { name: 'br', suffix: '.br' },
  { name: 'gzip', suffix: '.gz' },
] as const;

/** Whether an Accept-Encoding header allows `name` (named with q > 0, or covered by `*`). */
export function accepts(header: string | null, name: string): boolean {
  let any = false;
  for (const part of (header || '').toLowerCase().split(',')) {
    const [token, ...params] = part.split(';').map((s) => s.trim());
    const q = params.find((p) => p.startsWith('q='));
    const ok = !q || Number(q.slice(2)) > 0;
    if (token === name) return ok;
    if (token === '*') any = ok;
  }
  return any;
}

/**
 * The response for a static file that exists (`st` is its stat): its type and cache headers, and the
 * compressed copy the request accepts, if the build wrote one. A copy older than the file is ignored,
 * so a file changed after the build (e.g. by a test) is never answered with its stale copy.
 */
export async function staticFile(file: string, st: fs.Stats, rel: string, acceptEncoding: string | null, headers: Record<string, string> = {}): Promise<Response> {
  const type = path.extname(file);
  const immutable = /\.[0-9a-f]{8,}\.(js|css|png)$/.test(file) || rel.startsWith('/assets/');
  let body = file;
  if (COMPRESSED_TYPES.has(type)) {
    headers = { ...headers, vary: 'accept-encoding' };
    for (const e of ENCODINGS) {
      if (!accepts(acceptEncoding, e.name)) continue;
      const copy = await fs.promises.stat(file + e.suffix).catch(() => null);
      if (!copy?.isFile() || copy.mtimeMs < st.mtimeMs) continue;
      body = file + e.suffix;
      headers['content-encoding'] = e.name;
      break;
    }
  }
  return new Response(Bun.file(body), {
    headers: {
      'content-type': MIME[type] || 'application/octet-stream',
      'cache-control': immutable ? 'public, max-age=31536000, immutable' : 'no-cache',
      ...headers,
    },
  });
}
