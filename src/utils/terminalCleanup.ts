import type { PaneState, SplitNode } from '../types';

export function collectTerminalPanes(node: SplitNode): PaneState[] {
  return node.type === 'leaf' ? node.panes : node.children.flatMap(collectTerminalPanes);
}

/** Remove only confirmed PTYs, collapse empty splits, retain surviving proportions. */
export function pruneTerminals(node: SplitNode, removed: Set<number>): SplitNode | null {
  if (node.type === 'leaf') {
    const panes = node.panes.filter((p) => !removed.has(p.ptyId));
    if (!panes.length) return null;
    return { ...node, panes, activePaneId: panes.some((p) => p.id === node.activePaneId)
      ? node.activePaneId : panes[0].id };
  }
  const surviving = node.children.map((child, i) => ({ node: pruneTerminals(child, removed), size: node.sizes[i] ?? 1 }))
    .filter((entry): entry is { node: SplitNode; size: number } => entry.node !== null);
  if (!surviving.length) return null;
  if (surviving.length === 1) return surviving[0].node;
  const total = surviving.reduce((sum, entry) => sum + entry.size, 0);
  return { ...node, children: surviving.map((entry) => entry.node),
    sizes: surviving.map((entry) => total > 0 ? entry.size / total * 100 : 100 / surviving.length) };
}
