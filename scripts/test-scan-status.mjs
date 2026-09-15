import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
const source = await readFile(new URL('../src/scanStatus.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } });
const { scanStatus } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);
const scope = { id: 'a', availability: 'available', freshness: 'scanning' };
const run = { round: 1, scopeIds: ['a'], finished: false, phase: 'scanning', totalDirectories: 100, processedDirectories: 40, discoveredDirectories: 100, idleMs: 0 };
const state = (r = run, s = scope, paused = false) => scanStatus([s], [r], paused);
test('counting has no invented denominator', () => {
  const value = state({ ...run, phase: 'counting', totalDirectories: null });
  assert.equal(value.percent, null); assert.equal(value.title, '正在统计目录');
});
test('directory progress follows the fixed plan', () => assert.equal(state().percent, 40));
test('new folders wait for the next batch without changing this percentage', () => {
  const value = scanStatus([scope, { ...scope, id: 'b', freshness: 'unscanned' }], [run], false);
  assert.equal(value.percent, 40); assert.equal(value.waiting.length, 1);
});
test('earlier runs cannot inflate a rescan percentage', () => {
  const value = scanStatus([scope], [{ ...run, round: 1, finished: true, phase: 'complete', processedDirectories: 100 }, { ...run, round: 2, processedDirectories: 1 }], false);
  assert.equal(value.percent, 1);
});
test('unrelated active batches are not summed into a fake global percentage', () => {
  assert.equal(scanStatus([scope, { ...scope, id: 'b' }], [run, { ...run, round: 2, scopeIds: ['b'] }], false).percent, null);
});
test('all directories processed transitions to finalizing, never capped at 99', () => {
  const value = state({ ...run, processedDirectories: 100 });
  assert.equal(value.percent, null); assert.equal(value.title, '正在整理索引');
});
test('empty plan finalizes without division by zero', () => {
  const value = state({ ...run, totalDirectories: 0, processedDirectories: 0 });
  assert.equal(value.percent, null); assert.equal(value.title, '正在整理索引');
});
for (const [phase, title] of [['complete', '扫描完成'], ['partial', '扫描已结束，部分文件夹未完成'], ['failed', '扫描失败'], ['cancelled', '扫描已取消']]) {
  test(`terminal ${phase} has no running progress`, () => {
    const value = state({ ...run, finished: true, phase, processedDirectories: 100 }, { ...scope, freshness: phase === 'complete' ? 'current' : 'partial' });
    assert.equal(value.percent, null); assert.equal(value.title, title); assert.equal(value.busy, false);
  });
}
test('offline scopes cannot leave the interface waiting forever', () => {
  const value = scanStatus([{ ...scope, availability: 'offline' }], [], false);
  assert.equal(value.busy, false); assert.equal(value.warning, true);
});
test('pause keeps the measured percentage and suppresses stall warning', () => {
  const value = state({ ...run, idleMs: 70000 }, scope, true);
  assert.equal(value.title, '扫描已暂停'); assert.equal(value.percent, 40); assert.equal(value.stalled, false);
});
test('a blocked scan does not masquerade as normal progress', () => assert.equal(state({ ...run, idleMs: 70000 }).title, '扫描暂无进展'));
test('removed scopes do not leave stale runs on screen', () => assert.equal(scanStatus([scope], [{ ...run, scopeIds: ['removed'] }], false).run, undefined));
