import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

const script = new URL('../export-rational-app.mjs', import.meta.url);
for (const args of [[], ['--repo', 'git@github.com:shuaimu/rational.git'], ['--repo', 'https://github.com/shuaimu/rational'], ['--dry-run'], ['--no-push']]) {
  test(`retired export refuses ${JSON.stringify(args)} without invoking git`, () => {
    const result = spawnSync(process.execPath, [script.pathname, ...args], { encoding: 'utf8', env: { ...process.env, PATH: '' } });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /export is retired/);
    assert.match(result.stderr, /github.com\/shuaimu\/rational/);
    assert.equal(result.stdout, '');
  });
}
test('local checkout contents remain untouched, including independently maintained files', () => {
  const root = mkdtempSync(join(tmpdir(), 'retired-rational-'));
  try {
    mkdirSync(join(root, 'src'));
    writeFileSync(join(root, 'src', 'app.ts'), 'independent application\n');
    writeFileSync(join(root, 'README.md'), 'maintained here\n');
    for (const extra of [[], ['--dry-run'], ['--no-push']]) {
      const result = spawnSync(process.execPath, [script.pathname, '--dir', root, ...extra], { encoding: 'utf8' });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /export is retired/);
      assert.deepEqual(readdirSync(root).sort(), ['README.md', 'src']);
      assert.equal(readFileSync(join(root, 'src', 'app.ts'), 'utf8'), 'independent application\n');
      assert.equal(readFileSync(join(root, 'README.md'), 'utf8'), 'maintained here\n');
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});
