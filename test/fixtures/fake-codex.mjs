#!/usr/bin/env bun
// Fake `codex app-server` for tests: records every JSON-RPC method it receives and answers the
// initialize, rate-limit reads and reset-credit use. Usage: fake-codex.mjs app-server  (log path in FAKE_CODEX_LOG)
import fs from 'node:fs';
const log = process.env.FAKE_CODEX_LOG;
let buf = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (d) => {
  buf += d;
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i);
    buf = buf.slice(i + 1);
    let m;
    try {
      m = JSON.parse(line);
    } catch {
      continue;
    }
    if (log) fs.appendFileSync(log, m.method + '\n');
    if (m.method === 'initialize') process.stdout.write(JSON.stringify({ id: m.id, result: { userAgent: 'fake' } }) + '\n');
    else if (m.method === 'account/rateLimits/read') {
      if (process.env.FAKE_CODEX_FAIL) process.stdout.write(JSON.stringify({ id: m.id, error: { code: -1, message: 'rate limited: 429' } }) + '\n');
      else
        process.stdout.write(
          JSON.stringify({
            id: m.id,
            result: {
              rateLimits: {
                limitId: 'codex',
                primary: { usedPercent: 42, windowDurationMins: 300, resetsAt: Math.floor(Date.now() / 1000) + 3600 },
                secondary: { usedPercent: 7, windowDurationMins: 10080, resetsAt: Math.floor(Date.now() / 1000) - 60 },
                credits: { hasCredits: true, unlimited: false, balance: '12.50' },
                planType: 'pro',
              },
              rateLimitsByLimitId: {
                codex: { limitId: 'codex', primary: { usedPercent: 42, windowDurationMins: 300 } },
                codex_bengalfox: { limitId: 'codex_bengalfox', limitName: 'GPT-5.3-Codex-Spark', primary: { usedPercent: 3, windowDurationMins: 300, resetsAt: Math.floor(Date.now() / 1000) + 7200 } },
              },
              rateLimitResetCredits: { availableCount: 1, credits: [{ id: 'c1', status: 'available', resetType: 'codexRateLimits', title: 'Limit reset', grantedAt: Math.floor(Date.now() / 1000) - 60, expiresAt: Math.floor(Date.now() / 1000) + 86400 }] },
            },
          }) + '\n',
        );
    } else if (m.method === 'account/rateLimitResetCredit/consume') {
      if (log) fs.appendFileSync(log, 'KEY ' + (m.params?.idempotencyKey ? 'set' : 'missing') + '\n');
      process.stdout.write(JSON.stringify({ id: m.id, result: { outcome: process.env.FAKE_CODEX_RESET_OUTCOME || 'reset' } }) + '\n');
    } else if (m.method === 'thread/start' || m.method === 'turn/start') {
      if (log) fs.appendFileSync(log, 'TURN_STARTED\n');
    }
  }
});
