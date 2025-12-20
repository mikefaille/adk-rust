import { useCallback } from 'react';
import { useReactFlow } from '@xyflow/react';
import dagre from 'dagre';
import type { LayoutDirection } from '../types/layout';
import { useStore } from '../store';

export function useLayout() {
  const { getNodes, getEdges, setNodes, fitView } = useReactFlow();
  const layoutDirection = useStore(s => s.layoutDirection);
  const setLayoutDirection = useStore(s => s.setLayoutDirection);

  const applyLayout = useCallback(() => {
    const nodes = getNodes();
    const edges = getEdges();
    if (nodes.length === 0) return;

    // Toggle between TB and LR
    const newDirection: LayoutDirection = layoutDirection === 'LR' ? 'TB' : 'LR';
    setLayoutDirection(newDirection);

    const g = new dagre.graphlib.Graph();
    g.setGraph({ rankdir: newDirection, nodesep: 40, ranksep: 100 });
    g.setDefaultEdgeLabel(() => ({}));

    nodes.forEach(node => g.setNode(node.id, { width: 180, height: 100 }));
    edges.forEach(edge => g.setEdge(edge.source, edge.target));
    dagre.layout(g);

    setNodes(nodes.map(node => {
      const pos = g.node(node.id);
      return { ...node, position: { x: pos.x - 90, y: pos.y - 50 } };
    }));

    setTimeout(() => fitView({ padding: 0.2 }), 50);
  }, [getNodes, getEdges, setNodes, fitView, layoutDirection, setLayoutDirection]);

  const fitToView = useCallback(() => fitView({ padding: 0.2, duration: 300 }), [fitView]);

  return { applyLayout, fitToView, layoutDirection };
}
