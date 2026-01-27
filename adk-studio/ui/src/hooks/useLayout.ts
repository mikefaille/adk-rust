import { useCallback } from 'react';
import { useReactFlow } from '@xyflow/react';
import dagre from 'dagre';
import type { LayoutDirection, LayoutMode } from '../types/layout';
import { useStore } from '../store';

/**
 * Hook for managing canvas layout.
 * Supports both free-form and fixed (auto-layout) modes.
 * 
 * In free mode: Nodes can be placed anywhere, manual positioning
 * In fixed mode: Nodes are auto-arranged using Dagre layout
 * 
 * @see Requirements 2.1-2.10: Canvas Layout Modes
 * @see Requirements 8.5, 8.7, 8.10: Canvas Controls (fit-to-view, zoom)
 */
export function useLayout() {
  const { getNodes, getEdges, setNodes, fitView, zoomIn, zoomOut, getZoom } = useReactFlow();
  
  // Layout state from store
  const layoutMode = useStore(s => s.layoutMode);
  const layoutDirection = useStore(s => s.layoutDirection);
  const snapToGrid = useStore(s => s.snapToGrid);
  const gridSize = useStore(s => s.gridSize);
  const selectedNodeId = useStore(s => s.selectedNodeId);
  
  // Layout actions from store
  const setLayoutMode = useStore(s => s.setLayoutMode);
  const setLayoutDirection = useStore(s => s.setLayoutDirection);
  const setSnapToGrid = useStore(s => s.setSnapToGrid);
  const setGridSize = useStore(s => s.setGridSize);

  // Padding accounts for toolbar at top (~60px) and side panel (~320px) when node is selected
  const getPadding = useCallback(() => {
    // Top padding needs to be larger to account for the canvas toolbar
    // Right padding is larger when a node is selected (properties panel open)
    return { 
      top: 0.15,      // Extra padding for toolbar
      left: 0.1, 
      bottom: 0.1, 
      right: selectedNodeId ? 0.35 : 0.1 
    };
  }, [selectedNodeId]);

  // Apply Dagre auto-layout
  const doLayout = useCallback((direction: LayoutDirection) => {
    const nodes = getNodes();
    const edges = getEdges();
    if (nodes.length === 0) return;

    const g = new dagre.graphlib.Graph();
    g.setGraph({ rankdir: direction, nodesep: 40, ranksep: 100 });
    g.setDefaultEdgeLabel(() => ({}));

    nodes.forEach(node => g.setNode(node.id, { width: 180, height: 100 }));
    edges.forEach(edge => g.setEdge(edge.source, edge.target));
    dagre.layout(g);

    setNodes(nodes.map(node => {
      const pos = g.node(node.id);
      return { ...node, position: { x: pos.x - 90, y: pos.y - 50 } };
    }));

    setTimeout(() => fitView({ padding: getPadding(), maxZoom: 0.9 }), 50);
  }, [getNodes, getEdges, setNodes, fitView, getPadding]);

  // Toggle layout direction (TB <-> LR) and apply layout
  const toggleDirection = useCallback(() => {
    const newDirection: LayoutDirection = layoutDirection === 'LR' ? 'TB' : 'LR';
    setLayoutDirection(newDirection);
    // Always apply layout when direction is toggled
    doLayout(newDirection);
  }, [layoutDirection, setLayoutDirection, doLayout]);

  // Toggle layout mode (free <-> fixed)
  const toggleMode = useCallback(() => {
    const newMode: LayoutMode = layoutMode === 'free' ? 'fixed' : 'free';
    setLayoutMode(newMode);
    // Apply layout when switching to fixed mode
    if (newMode === 'fixed') {
      doLayout(layoutDirection);
    }
  }, [layoutMode, layoutDirection, setLayoutMode, doLayout]);

  // Legacy: Toggle layout (direction) - for backward compatibility
  const toggleLayout = useCallback(() => {
    toggleDirection();
  }, [toggleDirection]);

  // Apply layout without toggling (uses current direction)
  // Note: We read layoutDirection from the store directly to avoid stale closures
  // when applyLayout is called from setTimeout callbacks
  const applyLayout = useCallback(() => {
    const currentDirection = useStore.getState().layoutDirection;
    doLayout(currentDirection);
  }, [doLayout]);

  /**
   * Fit all nodes in view
   * @see Requirements 8.5, 8.10: Fit-to-view functionality
   * 
   * Ensures all nodes are visible within the viewport with appropriate padding.
   * Uses maxZoom to prevent over-zooming on small graphs.
   */
  const fitToView = useCallback(() => {
    fitView({ 
      padding: getPadding(), 
      duration: 300, 
      maxZoom: 0.9,
      // Ensure all nodes are included
      includeHiddenNodes: false,
    });
  }, [fitView, getPadding]);

  // Snap position to grid
  const snapPosition = useCallback((x: number, y: number): { x: number; y: number } => {
    if (!snapToGrid) return { x, y };
    return {
      x: Math.round(x / gridSize) * gridSize,
      y: Math.round(y / gridSize) * gridSize,
    };
  }, [snapToGrid, gridSize]);

  /**
   * Zoom in by a step
   * @see Requirements 8.7, 11.8: Keyboard shortcuts for zoom
   */
  const handleZoomIn = useCallback(() => {
    zoomIn({ duration: 200 });
  }, [zoomIn]);

  /**
   * Zoom out by a step
   * @see Requirements 8.7, 11.8: Keyboard shortcuts for zoom
   */
  const handleZoomOut = useCallback(() => {
    zoomOut({ duration: 200 });
  }, [zoomOut]);

  /**
   * Get current zoom level
   */
  const getCurrentZoom = useCallback(() => {
    return getZoom();
  }, [getZoom]);

  return {
    // State
    layoutMode,
    layoutDirection,
    snapToGrid,
    gridSize,
    
    // Mode actions
    setLayoutMode,
    toggleMode,
    
    // Direction actions
    setLayoutDirection,
    toggleDirection,
    toggleLayout, // Legacy alias for toggleDirection
    
    // Grid actions
    setSnapToGrid,
    setGridSize,
    snapPosition,
    
    // Layout actions
    applyLayout,
    fitToView,
    
    // Zoom actions (v2.0)
    // @see Requirements 8.7, 11.8
    zoomIn: handleZoomIn,
    zoomOut: handleZoomOut,
    getZoom: getCurrentZoom,
  };
}
