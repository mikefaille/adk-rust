import { useEffect, useRef } from 'react';
import { Node, Edge, useNodesState, useEdgesState } from '@xyflow/react';
import type { Project, Edge as WorkflowEdge } from '../types/project';
import { useStore } from '../store';

interface ExecutionState {
  activeAgent: string | null;
  iteration: number;
  flowPhase: 'idle' | 'input' | 'output';
  thoughts?: Record<string, string>;
  /** v2.0: State keys from SSE events for data flow overlays (nodeId -> keys) */
  stateKeys?: Map<string, string[]>;
  /** v2.0: Whether to show data flow overlay */
  showDataFlowOverlay?: boolean;
  /** v2.0: Currently highlighted state key (for hover highlighting) */
  highlightedKey?: string | null;
  /** v2.0: Callback when a state key is hovered */
  onKeyHover?: (key: string | null) => void;
  /** v2.0: Execution path for highlighting (ordered list of node IDs) */
  executionPath?: string[];
  /** v2.0: Whether execution is in progress */
  isExecuting?: boolean;
}

export function useCanvasNodes(project: Project | null, execution: ExecutionState) {
  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const { 
    activeAgent, 
    iteration, 
    flowPhase, 
    thoughts = {}, 
    stateKeys, 
    showDataFlowOverlay, 
    highlightedKey, 
    onKeyHover,
    executionPath = [],
    isExecuting = false,
  } = execution;
  const layoutDirection = useStore(s => s.layoutDirection);
  const isHorizontal = layoutDirection === 'LR' || layoutDirection === 'RL';
  
  // Track project structure for detecting actual changes
  const prevAgentKeys = useRef<string | null>(null);
  const prevToolsHash = useRef<string | null>(null);

  // Build nodes only when project STRUCTURE changes (agents added/removed)
  useEffect(() => {
    if (!project) return;
    const agentKeys = Object.keys(project.agents).sort().join(',');
    const toolsHash = Object.entries(project.agents).map(([id, a]) => `${id}:${a.tools?.join(',')}`).join('|');
    
    if (agentKeys === prevAgentKeys.current && toolsHash === prevToolsHash.current) return;
    prevAgentKeys.current = agentKeys;
    prevToolsHash.current = toolsHash;

    const agentIds = Object.keys(project.agents);
    
    // v2.0: If no agents, show empty canvas (no START/END nodes)
    // This allows undo to restore to a truly blank state
    if (agentIds.length === 0) {
      setNodes([]);
      return;
    }
    
    const allSubAgents = new Set(agentIds.flatMap(id => project.agents[id].sub_agents || []));
    const topLevelAgents = agentIds.filter(id => !allSubAgents.has(id));

    const sortedAgents: string[] = [];
    let current = 'START';
    while (sortedAgents.length < topLevelAgents.length) {
      const nextEdge = project.workflow.edges.find((e: WorkflowEdge) => e.from === current && e.to !== 'END');
      if (!nextEdge) break;
      if (topLevelAgents.includes(nextEdge.to)) sortedAgents.push(nextEdge.to);
      current = nextEdge.to;
    }
    topLevelAgents.forEach(id => { if (!sortedAgents.includes(id)) sortedAgents.push(id); });

    const newNodes: Node[] = [
      { id: 'START', position: { x: 50, y: 50 }, data: {}, type: 'start' },
      { id: 'END', position: { x: 50, y: 150 + sortedAgents.length * 150 }, data: {}, type: 'end' },
    ];

    sortedAgents.forEach((id, i) => {
      const agent = project.agents[id];
      const pos = { x: 50, y: 150 + i * 150 };
      const subAgentTools = (agent.sub_agents || []).reduce((acc, subId) => {
        acc[subId] = project.agents[subId]?.tools || [];
        return acc;
      }, {} as Record<string, string[]>);
      
      if (agent.type === 'sequential') newNodes.push({ id, type: 'sequential', position: pos, data: { label: id, subAgents: agent.sub_agents, subAgentTools } });
      else if (agent.type === 'loop') newNodes.push({ id, type: 'loop', position: pos, data: { label: id, subAgents: agent.sub_agents, subAgentTools, maxIterations: agent.max_iterations || 3 } });
      else if (agent.type === 'parallel') newNodes.push({ id, type: 'parallel', position: pos, data: { label: id, subAgents: agent.sub_agents, subAgentTools } });
      else if (agent.type === 'router') newNodes.push({ id, type: 'router', position: pos, data: { label: id, routes: agent.routes || [] } });
      else newNodes.push({ id, type: 'llm', position: pos, data: { label: id, model: agent.model, tools: agent.tools || [] } });
    });
    setNodes(newNodes);
  }, [project, setNodes]);

  // Update execution state (isActive, iteration, thoughts, execution path) WITHOUT changing positions
  useEffect(() => {
    if (!project) return;
    setNodes(nds => nds.map(n => {
      if (n.id === 'START' || n.id === 'END') {
        // v2.0: Add execution path highlighting for START/END nodes
        const isInPath = executionPath.includes(n.id);
        return {
          ...n,
          data: {
            ...n.data,
            isInExecutionPath: isInPath,
          },
          className: isInPath ? 'node-execution-path' : undefined,
        };
      }
      const agent = project.agents[n.id];
      if (!agent) return n;
      
      const isActive = activeAgent === n.id || (activeAgent && agent.sub_agents?.includes(activeAgent));
      const activeSub = activeAgent && agent.sub_agents?.includes(activeAgent) ? activeAgent : undefined;
      
      // v2.0: Check if node is in execution path
      // @see Requirement 10.5: Highlight execution path from start to current node
      const isInPath = executionPath.includes(n.id);
      
      return {
        ...n,
        data: {
          ...n.data,
          isActive,
          activeSubAgent: activeSub,
          currentIteration: agent.type === 'loop' ? iteration : undefined,
          thought: n.type === 'llm' ? thoughts[n.id] : undefined,
          isInExecutionPath: isInPath,
        },
        // Add CSS class for execution path styling
        className: isActive ? 'node-active' : (isInPath ? 'node-execution-path' : undefined),
      };
    }));
  }, [project, activeAgent, iteration, thoughts, executionPath, setNodes]);

  // Rebuild edges when project edges or layout direction changes
  useEffect(() => {
    if (!project) return;
    setEdges(project.workflow.edges.map((e: WorkflowEdge, i: number) => {
      const animated = (activeAgent && e.to === activeAgent) || (flowPhase === 'input' && e.from === 'START') || (flowPhase === 'output' && e.to === 'END');
      
      // v2.0: Get state keys for this edge from the source node
      // @see Requirements 3.3: State keys from runtime execution events
      const edgeStateKeys = stateKeys?.get(e.from) || [];
      
      // v2.0: Check if edge is in execution path
      // @see Requirement 10.3, 10.5: Highlight execution path
      const sourceIndex = executionPath.indexOf(e.from);
      const targetIndex = executionPath.indexOf(e.to);
      const isInPath = sourceIndex !== -1 && targetIndex !== -1 && targetIndex === sourceIndex + 1;
      const isAnimatedPath = isExecuting && animated;
      
      return { 
        id: `e${i}-${layoutDirection}`,
        source: e.from, 
        target: e.to, 
        // Use dataflow edge type when overlay is enabled, otherwise animated
        type: showDataFlowOverlay ? 'dataflow' : 'animated', 
        data: { 
          animated: animated || isAnimatedPath,
          // v2.0: Data flow overlay data
          stateKeys: edgeStateKeys,
          showOverlay: showDataFlowOverlay,
          highlightedKey,
          onKeyHover,
          // v2.0: Execution path data
          isExecutionPath: isInPath && !isAnimatedPath,
        },
        sourceHandle: isHorizontal ? 'right' : 'bottom',
        targetHandle: isHorizontal ? 'left' : 'top',
      };
    }));
  }, [project?.workflow.edges, flowPhase, activeAgent, setEdges, layoutDirection, isHorizontal, stateKeys, showDataFlowOverlay, highlightedKey, onKeyHover, executionPath, isExecuting]);

  return { nodes, edges, setNodes, setEdges, onNodesChange, onEdgesChange };
}
