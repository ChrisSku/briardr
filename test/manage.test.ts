// Renaming and closing workspaces, projects and streams through the API, checked in the test run's own
// Herdr session: what the API says has to be what happened there.
import assert from 'node:assert/strict';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { closeWorkspace, herdr, request, startServer, testPane, type TestServer } from './helpers.ts';

let s: TestServer;
let w: ReturnType<typeof testPane>;
let tab: string;
const enc = encodeURIComponent;
const post = (p: string, body?: unknown) => request(s, 'POST', p, body ?? {});
const has = (kind: 'workspace' | 'tab' | 'pane', id: string): boolean => herdr(kind, 'list').result[`${kind}s`].some((x: any) => x[`${kind}_id`] === id);

beforeAll(async () => {
  s = await startServer();
  w = testPane('manage');
  await s.run.herdr.refresh();
  tab = herdr('pane', 'get', w.pane).result.pane.tab_id;
});
afterAll(async () => {
  closeWorkspace(w.workspace);
  await s.close();
});

describe('renaming', () => {
  test('a workspace and a project take the new name in Herdr, cleaned of control characters', async () => {
    assert.equal((await post(`/api/workspaces/${enc(w.workspace)}/rename`, { name: '  new\u0007 ws ' })).status, 200);
    assert.equal(herdr('workspace', 'get', w.workspace).result.workspace.label, 'new ws');
    assert.equal((await post(`/api/projects/${enc(tab)}/rename`, { name: 'new project' })).status, 200);
    assert.equal(herdr('tab', 'get', tab).result.tab.label, 'new project');
  });

  test('they need a name, unlike a stream, which falls back to its tool’s', async () => {
    for (const p of [`/api/workspaces/${enc(w.workspace)}/rename`, `/api/projects/${enc(tab)}/rename`]) {
      const r = await post(p, { name: '   ' });
      assert.equal(r.status, 400);
      assert.equal(r.json.error, 'invalid_name');
    }
    assert.equal(herdr('workspace', 'get', w.workspace).result.workspace.label, 'new ws');
    assert.equal((await post(`/api/streams/${enc(w.pane)}/rename`, { name: '' })).status, 200);
  });

  test('something that no longer exists is a 404, and a device that isn’t paired gets nothing', async () => {
    for (const [p, code] of [['workspaces', 'workspace_gone'], ['projects', 'project_gone']]) {
      const r = await post(`/api/${p}/nope/rename`, { name: 'x' });
      assert.equal(r.status, 404);
      assert.equal(r.json.error, code);
    }
    const r = await request(s, 'POST', `/api/workspaces/${enc(w.workspace)}/close`, {}, { cookie: false });
    assert.equal(r.status, 401);
    assert.ok(has('workspace', w.workspace));
  });
});

describe('closing', () => {
  test('a stream, a project and a workspace close in Herdr and leave the model', async () => {
    const t = testPane('closing');
    await s.run.herdr.refresh();
    const second = herdr('pane', 'split', t.pane, '--direction', 'right', '--no-focus').result.pane.pane_id;
    const other = herdr('tab', 'create', '--workspace', t.workspace, '--no-focus').result;
    await s.run.herdr.refresh();

    assert.equal((await post(`/api/streams/${enc(second)}/close`)).status, 200);
    assert.ok(!has('pane', second));
    assert.ok(has('pane', t.pane), 'the stream beside it stays');
    assert.equal((await post(`/api/streams/${enc(second)}/close`)).status, 404, 'it’s gone now');

    assert.equal((await post(`/api/projects/${enc(other.tab.tab_id)}/close`)).status, 200);
    assert.ok(!has('tab', other.tab.tab_id));
    assert.ok(has('workspace', t.workspace), 'the workspace stays');

    assert.equal((await post(`/api/workspaces/${enc(t.workspace)}/close`)).status, 200);
    assert.ok(!has('workspace', t.workspace));
    assert.ok(!has('pane', t.pane));
    assert.equal((await post(`/api/workspaces/${enc(t.workspace)}/close`)).status, 404);
  });

  test('closing the last stream closes the workspace too, as the confirmation says', async () => {
    const t = testPane('last stream');
    await s.run.herdr.refresh();
    assert.equal((await post(`/api/streams/${enc(t.pane)}/close`)).status, 200);
    assert.ok(!has('workspace', t.workspace));
  });
});
