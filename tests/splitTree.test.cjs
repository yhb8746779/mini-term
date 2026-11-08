const assert = require('node:assert/strict');
// Importing shared types makes TypeScript preserve the src/utils directory.
const {
  findLeafByPaneId, removePaneFromTree, reorderPaneInLeaf,
  insertPaneIntoLeaf, splitLeafWithPane, movePane,
} = require('../.tmp-tests/utils/splitTree.js');

const pane = id => ({ id, ptyId: id.charCodeAt(0), shellName: 'sh', status: 'idle' });
const leaf = (...ids) => ({ type: 'leaf', panes: ids.map(pane), activePaneId: ids[0] });
const split = (...children) => ({ type: 'split', direction: 'horizontal', children, sizes: children.map(() => 100 / children.length) });
const ids = node => node.panes.map(p => p.id);
const frozen = value => {
  Object.freeze(value);
  Object.values(value).forEach(v => { if (v && typeof v === 'object') frozen(v); });
  return value;
};
const group = frozen(leaf('a', 'b', 'c'));
assert.deepEqual(ids(reorderPaneInLeaf(group, 'c', 0)), ['c', 'a', 'b']);
assert.deepEqual(ids(reorderPaneInLeaf(group, 'a', 3)), ['b', 'c', 'a']);
assert.equal(reorderPaneInLeaf(group, 'b', 1), group);
assert.equal(reorderPaneInLeaf(group, 'b', 2), group);
assert.equal(movePane(group, 'a', { kind: 'into', leafKey: 'a', index: 0 }), group);
assert.equal(movePane(group, 'missing', { kind: 'into', leafKey: 'a' }), group);
assert.equal(movePane(group, 'a', { kind: 'edge', leafKey: 'missing', edge: 'left' }), group);
assert.equal(findLeafByPaneId(group, 'b'), group);
assert.equal(findLeafByPaneId(group, 'missing'), null);
let removed = removePaneFromTree(group, 'a');
assert.equal(removed.pane, group.panes[0]);
assert.equal(removed.tree.activePaneId, 'b');
assert.equal(removePaneFromTree({ ...group, activePaneId: 'c' }, 'c').tree.activePaneId, 'b');
assert.equal(removePaneFromTree(group, 'b').tree.activePaneId, 'a');
assert.deepEqual(removePaneFromTree(leaf('a'), 'a'), { tree: null, pane: pane('a') });
assert.equal(removePaneFromTree(group, 'missing').tree, group);
const original = frozen(split(leaf('a'), leaf('b', 'c')));
const moved = movePane(original, 'a', { kind: 'into', leafKey: 'b', index: 1 });
assert.deepEqual(ids(moved), ['b', 'a', 'c']);
assert.equal(moved.activePaneId, 'a');
assert.equal(moved.panes[1], original.children[0].panes[0]);
const nested = frozen(split(split(leaf('a'), leaf('b')), leaf('c'), leaf('d')));
removed = removePaneFromTree(nested, 'a');
assert.equal(removed.tree.children[0].panes[0].id, 'b');
assert.deepEqual(removed.tree.sizes, nested.sizes);
removed = removePaneFromTree(split(leaf('a'), leaf('b'), leaf('c'), leaf('d')), 'b');
assert.deepEqual(removed.tree.sizes, [100 / 3, 100 / 3, 100 / 3]);
const retained = movePane(split(leaf('a', 'b'), leaf('c')), 'a', { kind: 'into', leafKey: 'c' });
assert.equal(retained.children[0].activePaneId, 'b');
assert.deepEqual(ids(retained.children[1]), ['c', 'a']);
assert.equal(retained.children[1].activePaneId, 'a');
const inserted = insertPaneIntoLeaf(group, 'b', pane('d'), 1);
assert.deepEqual(ids(inserted), ['a', 'd', 'b', 'c']);
assert.equal(inserted.activePaneId, 'd');
for (const edge of ['left', 'right', 'top', 'bottom']) {
  const before = edge === 'left' || edge === 'top';
  const result = movePane(original, 'a', { kind: 'edge', leafKey: 'b', edge });
  assert.equal(result.direction, edge === 'left' || edge === 'right' ? 'horizontal' : 'vertical');
  assert.deepEqual(result.sizes, [50, 50]);
  assert.deepEqual(ids(result.children[before ? 0 : 1]), ['a']);
  assert.deepEqual(ids(result.children[before ? 1 : 0]), ['b', 'c']);
  const sameGroup = movePane(group, 'a', { kind: 'edge', leafKey: 'a', edge });
  assert.deepEqual(ids(sameGroup.children[before ? 1 : 0]), ['b', 'c']);
  assert.equal(sameGroup.children[before ? 1 : 0].activePaneId, 'b');
  const single = leaf('a');
  assert.equal(movePane(single, 'a', { kind: 'edge', leafKey: 'a', edge }), single);
  assert.equal(movePane(single, 'a', { kind: 'into', leafKey: 'a' }), single);
  assert.equal(splitLeafWithPane(group, 'missing', pane('d'), edge), group);
}
const center = movePane(group, 'b', { kind: 'into', leafKey: 'a' });
assert.deepEqual(ids(center), ['a', 'c', 'b']);
assert.equal(center.activePaneId, 'b');
assert.equal(reorderPaneInLeaf(group, 'c', 0).activePaneId, 'a');
console.log('splitTree tests passed');
