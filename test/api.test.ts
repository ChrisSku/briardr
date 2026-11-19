// Requests to the host: a request given a signal stops when the signal aborts, at once when it was aborted
// before the call, and lets go of the signal when it ends, so one signal can serve any number of requests.
// Failures are ApiErrors whose message is shown as it is. host.ts and updates.ts need a browser (Svelte's
// runes), so api.ts gets stand-ins for the two things it uses from them; fetch and XMLHttpRequest are
// stand-ins too.
import assert from 'node:assert/strict';
import { afterAll, afterEach, beforeAll, beforeEach, mock, spyOn, test, type Mock } from 'bun:test';

let holds = 0;
mock.module('../web/src/host.ts', () => ({ theHost: () => 'the Mac', TheHost: () => 'The Mac' }));
mock.module('../web/src/updates.ts', () => ({ holdUpdates: () => (holds++, () => holds--) }));
const { api, ApiError, errorMessage, upload } = await import('../web/src/api.ts');

/** What fetch answers; nothing, and it never answers. */
let answer: () => Response | undefined;
/** The signal of each request fetch was given. */
let fetched: AbortSignal[];
/** fetch, failing at once when its signal aborts, as the real one does. */
function fakeFetch(_input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  return new Promise((resolve, reject) => {
    const signal = init!.signal!;
    fetched.push(signal);
    const fail = () => reject(new DOMException('aborted', 'AbortError'));
    if (signal.aborted) return fail();
    signal.addEventListener('abort', fail);
    const res = answer();
    if (res) resolve(res);
  });
}
let fetchSpy: Mock<typeof fetch>;
beforeEach(() => {
  fetched = [];
  answer = () => Response.json({ ok: true });
  fetchSpy = spyOn(globalThis, 'fetch').mockImplementation(fakeFetch as typeof fetch);
});
afterEach(() => fetchSpy.mockRestore());

/** How many listeners added to `signal` from now on are still on it, when the result is called. */
function listened(signal: AbortSignal) {
  const added = spyOn(signal, 'addEventListener');
  const removed = spyOn(signal, 'removeEventListener');
  return () => added.mock.calls.filter(([type, fn]) => !removed.mock.calls.some(([t, f]) => t === type && f === fn)).length;
}

async function failure(p: Promise<unknown>): Promise<InstanceType<typeof ApiError>> {
  const e = await p.then(
    () => assert.fail('expected the request to fail'),
    (e: unknown) => e,
  );
  assert.ok(e instanceof ApiError, String(e));
  return e;
}

test('a request answers with the body, and a failure with the host’s message', async () => {
  assert.deepEqual(await api('GET', '/api/x'), { ok: true });
  answer = () => Response.json({ error: 'stream_gone', message: 'That stream no longer exists.' }, { status: 404 });
  const e = await failure(api('GET', '/api/x'));
  assert.deepEqual([e.status, e.code, e.category, e.message], [404, 'stream_gone', 'client', 'That stream no longer exists.']);
});

test('a signal aborted before the call cancels the request at once', async () => {
  const ctrl = new AbortController();
  ctrl.abort();
  const e = await failure(api('GET', '/api/x', undefined, { signal: ctrl.signal }));
  assert.deepEqual([e.code, e.category], ['cancelled', 'client']);
  assert.equal(fetched[0].aborted, true, 'the request never went out');
});

test('aborting the signal cancels a request that is waiting for its answer', async () => {
  answer = () => undefined;
  const ctrl = new AbortController();
  const req = api('GET', '/api/x', undefined, { signal: ctrl.signal });
  ctrl.abort();
  assert.equal((await failure(req)).code, 'cancelled');
});

test('a request that gets no answer in time fails as a timeout', async () => {
  answer = () => undefined;
  const e = await failure(api('GET', '/api/x', undefined, { timeout: 5 }));
  assert.deepEqual([e.code, e.category, e.message], ['timeout', 'timeout', 'The Mac didn’t answer in time.']);
});

test('requests let go of the signal they were given, however they end', async () => {
  const ctrl = new AbortController();
  const held = listened(ctrl.signal);
  for (let i = 0; i < 3; i++) await api('GET', '/api/x', undefined, { signal: ctrl.signal });
  answer = () => Response.json({ error: 'nope' }, { status: 500 });
  await failure(api('GET', '/api/x', undefined, { signal: ctrl.signal }));
  answer = () => undefined;
  await failure(api('GET', '/api/x', undefined, { signal: ctrl.signal, timeout: 5 }));
  assert.equal(held(), 0);
  assert.equal(ctrl.signal.aborted, false);
});

test('a failure shows its own message, anything else thrown the fallback', () => {
  assert.equal(errorMessage(new ApiError(0, 'network', 'Can’t reach the Mac.', 'network'), 'Couldn’t save.'), 'Can’t reach the Mac.');
  assert.equal(errorMessage(new TypeError('x is undefined'), 'Couldn’t save.'), 'Couldn’t save.');
  assert.equal(errorMessage(undefined, 'Couldn’t save.'), 'Couldn’t save.');
});

// ---------- uploads ----------

/** An XMLHttpRequest that sends nothing: the test finishes the upload, and abort() ends one that was sent. */
class FakeXhr {
  static made: FakeXhr[] = [];
  sent = false;
  status = 0;
  responseText = '';
  withCredentials = false;
  upload: { onprogress: ((e: ProgressEvent) => void) | null } = { onprogress: null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  constructor() {
    FakeXhr.made.push(this);
  }
  open() {}
  send() {
    this.sent = true;
  }
  abort() {
    if (this.sent) this.onabort?.();
  }
  finish(status: number, body: unknown) {
    this.status = status;
    this.responseText = JSON.stringify(body);
    this.onload?.();
  }
}
const realXhr = globalThis.XMLHttpRequest;
beforeAll(() => (globalThis.XMLHttpRequest = FakeXhr as unknown as typeof XMLHttpRequest));
afterAll(() => (globalThis.XMLHttpRequest = realXhr));
beforeEach(() => (FakeXhr.made = []));

const send = (signal: AbortSignal) => upload('/api/streams/s1/uploads', new Blob(['x']), () => {}, signal);

test('an upload whose signal was aborted before the call is cancelled without sending', async () => {
  const ctrl = new AbortController();
  ctrl.abort();
  assert.equal((await failure(send(ctrl.signal))).code, 'cancelled');
  assert.equal(FakeXhr.made.some((x) => x.sent), false);
  assert.equal(holds, 0, 'updates no longer wait for it');
});

test('aborting the signal cancels an upload under way', async () => {
  const ctrl = new AbortController();
  const req = send(ctrl.signal);
  assert.equal(holds, 1, 'an update waits for the upload');
  ctrl.abort();
  assert.equal((await failure(req)).code, 'cancelled');
  assert.equal(holds, 0);
});

test('uploads let go of the signal they were given', async () => {
  const ctrl = new AbortController();
  const held = listened(ctrl.signal);
  const done = send(ctrl.signal);
  FakeXhr.made[0].finish(200, { path: 'a.png' });
  assert.deepEqual(await done, { path: 'a.png' });
  const refused = send(ctrl.signal);
  FakeXhr.made[1].finish(413, { error: 'too_big', message: 'That file is too big.' });
  assert.equal((await failure(refused)).message, 'That file is too big.');
  assert.equal(held(), 0);
});
