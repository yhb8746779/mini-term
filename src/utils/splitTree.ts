import type { PaneState, SplitNode } from '../types';

type Leaf = Extract<SplitNode, { type: 'leaf' }>;
export type Edge = 'left' | 'right' | 'top' | 'bottom';
export type MoveTarget =
  | { kind: 'reorder' | 'into'; leafKey: string; index?: number }
  | { kind: 'edge'; leafKey: string; edge: Edge };

export function findLeafByPaneId(node: SplitNode, paneId: string): Leaf | null {
  if (node.type === 'leaf') return node.panes.some(p => p.id === paneId) ? node : null;
  for (const child of node.children) {
    const leaf = findLeafByPaneId(child, paneId);
    if (leaf) return leaf;
  }
  return null;
}

function updateLeaf(node: SplitNode, key: string, update: (leaf: Leaf) => SplitNode): SplitNode {
  if (node.type === 'leaf') return node.panes.some(p => p.id === key) ? update(node) : node;
  const children = node.children.map(child => updateLeaf(child, key, update));
  return children.every((child, i) => child === node.children[i]) ? node : { ...node, children };
}

export function removePaneFromTree(node: SplitNode, paneId: string): { tree: SplitNode | null; pane: PaneState | null } {
  if (node.type === 'leaf') {
    const index = node.panes.findIndex(p => p.id === paneId);
    if (index < 0) return { tree: node, pane: null };
    const panes = node.panes.filter(p => p.id !== paneId);
    return {
      pane: node.panes[index],
      tree: panes.length ? { ...node, panes, activePaneId: node.activePaneId === paneId
        ? panes[Math.min(index, panes.length - 1)].id : node.activePaneId } : null,
    };
  }
  for (let i = 0; i < node.children.length; i++) {
    const removed = removePaneFromTree(node.children[i], paneId);
    if (!removed.pane) continue;
    const children = [...node.children];
    if (removed.tree) children[i] = removed.tree;
    else children.splice(i, 1);
    return { pane: removed.pane, tree: children.length === 0 ? null : children.length === 1 ? children[0] : {
      ...node, children, sizes: removed.tree ? node.sizes : children.map(() => 100 / children.length),
    } };
  }
  return { tree: node, pane: null };
}

// targetIndex is an insertion boundary in the original tab bar (0..panes.length).
export function reorderPaneInLeaf(node: SplitNode, paneId: string, targetIndex: number): SplitNode {
  return updateLeaf(node, paneId, leaf => {
    const from = leaf.panes.findIndex(p => p.id === paneId);
    const boundary = Math.max(0, Math.min(targetIndex, leaf.panes.length));
    const to = boundary > from ? boundary - 1 : boundary;
    if (to === from) return leaf;
    const panes = [...leaf.panes];
    const [pane] = panes.splice(from, 1);
    panes.splice(to, 0, pane);
    return { ...leaf, panes };
  });
}

export function insertPaneIntoLeaf(node: SplitNode, leafKey: string, pane: PaneState, index?: number): SplitNode {
  return updateLeaf(node, leafKey, leaf => {
    const panes = [...leaf.panes];
    panes.splice(Math.max(0, Math.min(index ?? panes.length, panes.length)), 0, pane);
    return { ...leaf, panes, activePaneId: pane.id };
  });
}

export function splitLeafWithPane(node: SplitNode, leafKey: string, pane: PaneState, edge: Edge): SplitNode {
  return updateLeaf(node, leafKey, leaf => {
    const newLeaf: Leaf = { type: 'leaf', panes: [pane], activePaneId: pane.id };
    return { type: 'split', direction: edge === 'left' || edge === 'right' ? 'horizontal' : 'vertical',
      children: edge === 'left' || edge === 'top' ? [newLeaf, leaf] : [leaf, newLeaf], sizes: [50, 50] };
  });
}

export function movePane(tree: SplitNode, paneId: string, target: MoveTarget): SplitNode {
  const source = findLeafByPaneId(tree, paneId);
  const destination = findLeafByPaneId(tree, target.leafKey);
  if (!source || !destination) return tree;
  if (source === destination && target.kind !== 'edge') {
    const reordered = reorderPaneInLeaf(tree, paneId, target.index ?? destination.panes.length);
    if (reordered === tree || target.kind === 'reorder') return reordered;
    return updateLeaf(reordered, paneId, leaf => ({ ...leaf, activePaneId: paneId }));
  }
  const stableKey = destination.panes.find(p => p.id !== paneId)?.id;
  if (!stableKey) return tree;
  const { tree: remaining, pane } = removePaneFromTree(tree, paneId);
  if (!remaining || !pane) return tree;
  return target.kind === 'edge' ? splitLeafWithPane(remaining, stableKey, pane, target.edge)
    : insertPaneIntoLeaf(remaining, stableKey, pane, target.index);
}
