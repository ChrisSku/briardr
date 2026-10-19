// Briardr's backend protocol for Bun: newline-delimited JSON on stdin and stdout. Briardr starts this
// process on the plugin's first request, sends each request from a device to it, and stops it by
// closing stdin. stdout is the protocol, so console.log goes to the log (stderr) instead.
// The log is in Briardr's data folder: logs/plugins/<plugin id>.log.

export interface Stream {
  id: string;
  name: string;
  kind: 'agent' | 'shell' | 'editor' | 'program';
  tool: string;
  state: 'blocked' | 'working' | 'done' | 'idle' | 'unknown' | null;
  /** The stream's folder (the pane's working directory); null when Herdr hasn't reported one. */
  cwd: string | null;
  folder: string | null;
  [key: string]: unknown;
}

export interface BriardrRequest {
  method: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
  /** The path after /api/plugins/<id>, e.g. "/items/3". */
  path: string;
  query: Record<string, string>;
  /** A JSON request body, parsed; null without one. */
  body: unknown;
  /** A non-JSON body: text, or base64 when it isn't UTF-8. */
  text?: string;
  base64?: string;
  /** The stream named by `?stream=<id>`, as Briardr knows it. */
  stream: Stream | null;
  /** The signed-in device that asked. */
  device: { id: string; name: string };
}

export interface BriardrResponse {
  status?: number;
  /** Only content-type, cache-control, content-disposition, etag and last-modified are passed on. */
  headers?: Record<string, string>;
  /** Sent as JSON. Or `text`, or `base64` for binary data. */
  body?: unknown;
  text?: string;
  base64?: string;
}

/** Data this plugin keeps on the host (a folder only it uses). */
export const dataDir = process.env.BRIARDR_PLUGIN_DATA || '.';

const write = (msg: object) => process.stdout.write(JSON.stringify(msg) + '\n');
console.log = console.error;
console.info = console.error;

/** Pushes an event to every device; plugin clients get it with `briardr.onEvent(topic, fn)`. */
export function emit(topic: string, data: unknown = null) {
  write({ type: 'event', topic, data });
}

/** Answers Briardr's requests with `handler` until Briardr closes stdin. */
export async function serve(handler: (req: BriardrRequest) => BriardrResponse | Promise<BriardrResponse>) {
  const decoder = new TextDecoder();
  let buffered = '';
  for await (const chunk of Bun.stdin.stream()) {
    buffered += decoder.decode(chunk, { stream: true });
    let nl: number;
    while ((nl = buffered.indexOf('\n')) >= 0) {
      const line = buffered.slice(0, nl);
      buffered = buffered.slice(nl + 1);
      if (line.trim()) void answer(line, handler);
    }
  }
  process.exit(0);
}

async function answer(line: string, handler: (req: BriardrRequest) => BriardrResponse | Promise<BriardrResponse>) {
  let id: number | undefined;
  try {
    const req = JSON.parse(line);
    id = req.id;
    const res = await handler(req as BriardrRequest);
    write({ type: 'response', id, status: 200, ...res });
  } catch (e) {
    console.error(e);
    write({ type: 'response', id, status: 500, body: { error: 'plugin_error', message: e instanceof Error ? e.message : String(e) } });
  }
}
