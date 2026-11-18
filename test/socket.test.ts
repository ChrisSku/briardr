// The socket under the live model and each terminal: a connection that's up is pinged every 10 s and is lost
// when a pong takes longer than 5 s; after a loss it connects again after a wait that grows with each failed
// attempt, and a returning network ends the wait. Nothing is sent while it isn't open, then or later, and a
// connection that was replaced or closed says nothing more.
import assert from 'node:assert/strict';
import { afterEach, beforeEach, jest, spyOn, test, type Mock } from 'bun:test';
import { ReconnectingSocket } from '../web/src/socket.ts';

/** A browser WebSocket, which the test drives as the server and the network. */
class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static made: FakeWebSocket[] = [];
  readyState = FakeWebSocket.CONNECTING;
  binaryType = 'blob';
  sent: any[] = [];
  /** The code this side closed it with. */
  closedWith: number | null = null;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onclose: ((e: { code: number }) => void) | null = null;
  constructor() {
    FakeWebSocket.made.push(this);
  }
  send(data: string) {
    if (this.readyState !== FakeWebSocket.OPEN) throw new Error('not open');
    this.sent.push(JSON.parse(data));
  }
  close(code: number) {
    this.closedWith = code;
    this.readyState = FakeWebSocket.CLOSED;
  }
  accept() {
    this.readyState = FakeWebSocket.OPEN;
    this.onopen?.();
  }
  say(msg: unknown) {
    this.onmessage?.({ data: typeof msg === 'string' || msg instanceof ArrayBuffer ? msg : JSON.stringify(msg) });
  }
  drop(code: number) {
    this.readyState = FakeWebSocket.CLOSED;
    this.onclose?.({ code });
  }
}

const realWebSocket = globalThis.WebSocket;
let random: Mock<() => number>;
beforeEach(() => {
  random = spyOn(Math, 'random').mockReturnValue(0.5); // the waits without jitter
  jest.useFakeTimers();
  FakeWebSocket.made = [];
  (globalThis as any).WebSocket = FakeWebSocket;
  (globalThis as any).window = new EventTarget(); // where the network's online event fires
});
afterEach(() => {
  random.mockRestore();
  jest.useRealTimers();
  globalThis.WebSocket = realWebSocket;
  delete (globalThis as any).window;
});

/** A socket and what reaches its caller. Its caller connects again when asked, as live.ts and the terminal do. */
function socket() {
  const log: unknown[] = [];
  const connect = () =>
    s.open('wss://host/ws/test', {
      opened: () => log.push('opened'),
      message: (msg) => log.push(msg),
      frame: (data) => log.push([...data]),
      lost: (code) => log.push({ lost: code }),
    });
  const s = new ReconnectingSocket({ name: 'test', maxWait: 4000, reconnect: connect });
  return { s, log, connect, ws: () => FakeWebSocket.made.at(-1)! };
}

const online = () => window.dispatchEvent(new Event('online'));

/** Whether p has settled once the jobs already queued have run. */
async function settled(p: Promise<unknown>): Promise<boolean> {
  let done = false;
  void p.then(() => (done = true));
  await Promise.resolve();
  await Promise.resolve();
  return done;
}

test('a connection that’s up is pinged every 10 s, and lost and closed when a pong takes longer than 5 s', () => {
  const { s, log, connect, ws } = socket();
  connect();
  const a = ws();
  a.accept();
  jest.advanceTimersByTime(30_000);
  assert.equal(a.sent.length, 0, 'not pinged before the caller says it’s up');
  s.up();
  jest.advanceTimersByTime(9_999);
  assert.equal(a.sent.length, 0);
  jest.advanceTimersByTime(1);
  assert.equal(a.sent[0].t, 'ping');
  jest.advanceTimersByTime(4_999);
  a.say({ t: 'pong', id: a.sent[0].id });
  jest.advanceTimersByTime(5_001);
  assert.equal(a.sent.length, 2, 'pinged again');
  jest.advanceTimersByTime(4_999);
  assert.deepEqual(log, ['opened', { t: 'pong', id: a.sent[0].id }], 'the pong reaches the caller too');
  jest.advanceTimersByTime(1);
  assert.deepEqual(log.at(-1), { lost: null });
  assert.equal(a.closedWith, 1000);
  jest.advanceTimersByTime(60_000);
  assert.equal(a.sent.length, 2, 'a lost connection isn’t pinged');
  assert.equal(FakeWebSocket.made.length, 1, 'nor connected again unless the caller retries');
});

test('the caller can check at once that a connection still answers', () => {
  const { s, log, connect, ws } = socket();
  connect();
  ws().accept();
  s.ping();
  assert.equal(ws().sent[0].t, 'ping');
  jest.advanceTimersByTime(5_000);
  assert.deepEqual(log.at(-1), { lost: null });
});

test('each retry waits twice as long as the one before, up to the cap; a connection that’s up starts the waits over', () => {
  const { s, log, connect, ws } = socket();
  connect();
  ws().drop(1006);
  assert.deepEqual(log, [{ lost: 1006 }]);
  assert.equal(s.retry(), 500);
  jest.advanceTimersByTime(499);
  assert.equal(FakeWebSocket.made.length, 1);
  jest.advanceTimersByTime(1);
  assert.equal(FakeWebSocket.made.length, 2);
  const waits = [];
  for (let i = 0; i < 4; i++) {
    ws().drop(1006);
    waits.push(s.retry());
    jest.advanceTimersByTime(waits[i]);
  }
  assert.deepEqual(waits, [1000, 2000, 4000, 4000]);
  assert.equal(s.attempts, 5);
  ws().accept();
  s.up();
  assert.equal(s.attempts, 0);
  ws().drop(1006);
  assert.equal(s.retry(), 500);
});

test('nothing is sent while the connection isn’t open, and nothing is kept to send later', () => {
  const { s, connect, ws } = socket();
  connect();
  assert.equal(s.connected, true);
  assert.equal(s.isOpen, false);
  assert.equal(s.send({ t: 'input', text: 'while connecting' }), false);
  ws().accept();
  assert.equal(s.isOpen, true);
  assert.equal(s.send({ t: 'input', text: 'while open' }), true);
  ws().drop(1006);
  assert.equal(s.connected, false);
  assert.equal(s.send({ t: 'input', text: 'while down' }), false);
  s.retry();
  jest.advanceTimersByTime(500);
  ws().accept();
  assert.deepEqual(FakeWebSocket.made.flatMap((w) => w.sent), [{ t: 'input', text: 'while open' }]);
});

test('a connection that was replaced or closed says nothing more; close() also ends a wait to retry', () => {
  const { s, log, connect, ws } = socket();
  connect();
  const a = ws();
  a.accept();
  connect();
  const b = ws();
  assert.equal(a.closedWith, 1000);
  a.say({ t: 'late' });
  a.drop(1006);
  b.accept();
  b.say({ t: 'hello' });
  s.close();
  assert.equal(b.closedWith, 1000);
  b.say({ t: 'late' });
  b.drop(1006);
  assert.deepEqual(log, ['opened', 'opened', { t: 'hello' }]);
  connect();
  ws().drop(1006);
  s.retry();
  s.close();
  jest.advanceTimersByTime(60_000);
  assert.equal(FakeWebSocket.made.length, 3);
});

test('JSON text and binary frames reach the caller; text that isn’t JSON doesn’t', () => {
  const { log, connect, ws } = socket();
  connect();
  const a = ws();
  assert.equal(a.binaryType, 'arraybuffer');
  a.accept();
  a.say('not JSON');
  a.say({ t: 'model', n: 1 });
  a.say(new Uint8Array([1, 0, 10]).buffer);
  assert.deepEqual(log, ['opened', { t: 'model', n: 1 }, [1, 0, 10]]);
});

test('a returning network starts the waits over and ends a wait, or a retry still connecting, at once', () => {
  const { s, connect, ws } = socket();
  connect();
  ws().drop(1006);
  for (let i = 0; i < 3; i++) {
    jest.advanceTimersByTime(s.retry());
    ws().drop(1006);
  }
  assert.equal(s.retry(), 4000);
  online();
  assert.equal(FakeWebSocket.made.length, 5, 'connected again without waiting');
  assert.equal(s.attempts, 0);
  const connecting = ws();
  online();
  assert.equal(connecting.closedWith, 1000, 'a retry still connecting starts over');
  assert.equal(FakeWebSocket.made.length, 6);
  ws().accept();
  s.up();
  online();
  assert.equal(FakeWebSocket.made.length, 6, 'a connection that’s up stays');
});

test('the network connects nothing the caller didn’t retry: a connection refused for good, or closed', () => {
  const { s, connect, ws } = socket();
  connect();
  ws().drop(1006);
  jest.advanceTimersByTime(s.retry());
  ws().drop(4401); // signed out: the caller stops here
  online();
  assert.equal(FakeWebSocket.made.length, 2);
  connect();
  s.close();
  online();
  assert.equal(FakeWebSocket.made.length, 3);
  const removed = spyOn(window, 'removeEventListener');
  s.dispose();
  assert.ok(removed.mock.calls.some(([type]) => type === 'online'), 'dispose() stops listening');
});

test('ended() resolves when the connection ends at either side, or is replaced, or after its wait', async () => {
  const { s, connect, ws } = socket();
  assert.ok(await settled(s.ended(2000)), 'no connection: at once');
  connect();
  ws().accept();
  const byServer = s.ended(2000);
  ws().say({ t: 'closed', category: 'released' });
  assert.ok(!(await settled(byServer)));
  ws().drop(4011);
  assert.ok(await settled(byServer));
  connect();
  const byReplacing = s.ended(2000);
  connect();
  assert.ok(await settled(byReplacing));
  const byWaiting = s.ended(2000);
  jest.advanceTimersByTime(1_999);
  assert.ok(!(await settled(byWaiting)));
  jest.advanceTimersByTime(1);
  assert.ok(await settled(byWaiting));
});
