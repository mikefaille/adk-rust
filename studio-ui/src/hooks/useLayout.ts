import { useCallback } from 'react';
import { useReactFlow, Node, Edge } from '@xyflow/react';
import dagre from 'dagre';
import type { LayoutConfig, LayoutMode } from '../types/layout';
import { getLayoutConfig } from '../layout';

export function useLayout() {
  const { getNodes, getEdges, setNodes, fitView } = useReactFlow();

  const detectMode = useCallback((nodes: Node[], edges: Edge[]): LayoutMode => {
    const hasRouter = nodes.some(n => n.type === 'router');
    const isLinear = edges.length === nodes.length - 1;
    if (isLinear && !hasRouter) return 'pipeline';
    if (hasRouter) return 'tree';
    return 'freeform';
  }, []);

  const applyLayout = useCallback((config: Partial<LayoutConfig> = {}) => {
    const nodes = getNodes();
    const edges = getEdges();
    if (nodes.length === 0) return;

    const mode = detectMode(nodes, edges);
    const { direction, nodeSpacing, rankSpacing } = getLayoutConfig(mode, config);

    const g = new dagre.graphlib.Graph();
    g.setGraph({ rankdir: direction, nodesep: nodeSpacing, ranksep: rankSpacing });
    g.setDefaultEdgeLabel(() => ({}));

    nodes.forEach(node => g.setNode(node.id, { width: 180, height: 100 }));
    edges.forEach(edge => g.setEdge(edge.source, edge.target));
    dagre.layout(g);

    setNodes(nodes.map(node => {
      const pos = g.node(node.id);
      return { ...node, position: { x: pos.x - 90, y: pos.y - 50 } };
    }));

    setTimeout(() => fitView({ padding: 0.2 }), 50);
  }, [getNodes, getEdges, setNodes, fitView, detectMode]);

  const fitToView = useCallback(() => fitView({ padding: 0.2, duration: 300 }), [fitView]);

  return { applyLayout, fitToView, detectMode };
}
