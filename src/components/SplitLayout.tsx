import { useRef } from 'react';
import { Allotment } from 'allotment';
import { PaneGroup } from './PaneGroup';
import type { SplitNode } from '../types';

interface Props {
  node: SplitNode;
  projectPath: string;
  /** Whether this node lives inside a split (i.e. has siblings). */
  isSplit?: boolean;
  onSplit?: (paneId: string, direction: 'horizontal' | 'vertical') => void;
  onCloseLeaf?: (node: SplitNode) => void;
  onUpdateNode?: (updated: SplitNode) => void;
  onTabDrop?: (sourceTabId: string, targetPaneId: string, direction: 'horizontal' | 'vertical', position: 'before' | 'after') => void;
  onLayoutChange?: (updatedNode: SplitNode) => void;
}

function getNodeKey(node: SplitNode): string {
  if (node.type === 'leaf') return node.panes.map((p) => p.id).join('+');
  return node.children.map(getNodeKey).join('-');
}

export function SplitLayout({ node, projectPath, isSplit, onSplit, onCloseLeaf, onUpdateNode, onTabDrop, onLayoutChange }: Props) {
  const rafRef = useRef<number>(0);
  const nodeRef = useRef(node);
  nodeRef.current = node;

  if (node.type === 'leaf') {
    return (
      <PaneGroup
        node={node}
        projectPath={projectPath}
        onSplit={onSplit ?? (() => {})}
        onClosePane={() => onCloseLeaf?.(node)}
        onUpdateNode={(updated) => onUpdateNode?.(updated)}
        onTabDrop={onTabDrop}
        isSplit={!!isSplit}
      />
    );
  }

  const handleSizesChange = (sizes: number[]) => {
    if (!onLayoutChange) return;
    cancelAnimationFrame(rafRef.current);
    rafRef.current = requestAnimationFrame(() => {
      const currentNode = nodeRef.current;
      if (currentNode.type !== 'split' || sizes.length !== currentNode.children.length) return;
      const total = sizes.reduce((a, b) => a + b, 0);
      const proportional = total > 0 ? sizes.map((s) => (s / total) * 100) : sizes;
      onLayoutChange({ ...currentNode, sizes: proportional });
    });
  };

  const handleChildLayoutChange = (index: number, updatedChild: SplitNode) => {
    if (!onLayoutChange) return;
    const currentNode = nodeRef.current;
    if (currentNode.type !== 'split') return;
    const newChildren = [...currentNode.children];
    newChildren[index] = updatedChild;
    onLayoutChange({ ...currentNode, children: newChildren });
  };

  const handleChildClose = (index: number) => {
    const currentNode = nodeRef.current;
    if (currentNode.type !== 'split') return;
    const remaining = currentNode.children.filter((_, i) => i !== index);
    if (remaining.length === 0) {
      onCloseLeaf?.(currentNode);
    } else if (remaining.length === 1) {
      // Collapse: promote the single remaining child
      onUpdateNode?.(remaining[0]);
    } else {
      onUpdateNode?.({
        ...currentNode,
        children: remaining,
        sizes: remaining.map(() => 100 / remaining.length),
      });
    }
  };

  const handleChildUpdate = (index: number, updated: SplitNode) => {
    const currentNode = nodeRef.current;
    if (currentNode.type !== 'split') return;
    const newChildren = [...currentNode.children];
    newChildren[index] = updated;
    // Use onUpdateNode for structural changes (tab add/remove/switch in PaneGroup)
    // to bypass the pane-ID validation in handleLayoutChange.
    onUpdateNode?.({ ...currentNode, children: newChildren });
  };

  return (
    <Allotment
      vertical={node.direction === 'vertical'}
      defaultSizes={node.sizes}
      onChange={handleSizesChange}
    >
      {node.children.map((child, index) => (
        <Allotment.Pane key={getNodeKey(child)}>
          <SplitLayout
            node={child}
            projectPath={projectPath}
            isSplit={true}
            onSplit={onSplit}
            onCloseLeaf={() => handleChildClose(index)}
            onUpdateNode={(updated) => handleChildUpdate(index, updated)}
            onTabDrop={onTabDrop}
            onLayoutChange={(updated) => handleChildLayoutChange(index, updated)}
          />
        </Allotment.Pane>
      ))}
    </Allotment>
  );
}
