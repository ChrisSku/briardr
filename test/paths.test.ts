// Folders are shown with the home folder as ~, whatever the host's system.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { shortPath } from '../web/src/paths.ts';

test('the home folder is ~ on a Mac and on Linux', () => {
  assert.equal(shortPath('/Users/sam/Code/app'), '~/Code/app');
  assert.equal(shortPath('/home/sam/code/app'), '~/code/app');
  assert.equal(shortPath('/root/app'), '~/app');
  assert.equal(shortPath('/home/sam'), '~');
});

test('other folders stay as they are', () => {
  for (const p of ['/srv/app', '/rootfs/app', '/home', '/var/home/sam/app', '/opt/Users/sam']) assert.equal(shortPath(p), p);
});
