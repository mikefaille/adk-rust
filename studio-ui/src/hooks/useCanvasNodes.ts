import { useEffect } from 'react';
import { Node, Edge, useNodesState, useEdgesState } from '@xyflow/react';
import type { Project, Edge as WorkflowEdge } from '../types/project';
import { useStore } from '../store';

interface ExecutionState {
  activeAgent: string | null;
  iteration: number;
  flowPhase: 'idle' | 'input' | 'output';
  thoughts?: Record<string, string>;
}

export function useCanvasNodes(project: Project | null, execution: ExecutionState) {
  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const { activeAgent, iteration, flowPhase, thoughts = {} } = execution;
  const layoutDirection = useStore(s => s.layoutDirection);
  const isHorizontal = layoutDirection === 'LR' || layoutDirection === 'RL';

  // Build nodes when project or active agent changes
  useEffect(() => {
    if (!project) return;
    const agentIds = Object.keys(project.agents);
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
      const isActive = activeAgent === id || (activeAgent && agent.sub_agents?.includes(activeAgent));
      const activeSub = activeAgent && agent.sub_agents?.includes(activeAgent) ? activeAgent : undefined;

      if (agent.type === 'sequential') newNodes.push({ id, type: 'sequential', position: pos, data: { label: id, subAgents: agent.sub_agents, activeSubAgent: activeSub, isActive } });
      else if (agent.type === 'loop') newNodes.push({ id, type: 'loop', position: pos, data: { label: id, subAgents: agent.sub_agents, maxIterations: agent.max_iterations || 3, currentIteration: iteration, activeSubAgent: activeSub, isActive } });
      else if (agent.type === 'parallel') newNodes.push({ id, type: 'parallel', position: pos, data: { label: id, subAgents: agent.sub_agents, activeSubAgent: activeSub, isActive } });
      else if (agent.type === 'router') newNodes.push({ id, type: 'router', position: pos, data: { label: id, routes: agent.routes || [], isActive: activeAgent === id } });
      else newNodes.push({ id, type: 'llm', position: pos, data: { label: id, model: agent.model, tools: agent.tools || [], isActive: activeAgent === id } });
    });
    setNodes(newNodes);
  }, [project, setNodes, activeAgent, iteration]);

  // Update thoughts separately - lightweight update
  useEffect(() => {
    setNodes(nds => nds.map(n => 
      n.type === 'llm' ? { ...n, data: { ...n.data, thought: thoughts[n.id] } } : n
    ));
  }, [thoughts, setNodes]);

  // Rebuild edges when project, execution state, or layout direction changes
  useEffect(() => {
    if (!project) return;
    setEdges(project.workflow.edges.map((e: WorkflowEdge, i: number) => {
      const animated = (activeAgent && e.to === activeAgent) || (flowPhase === 'input' && e.from === 'START') || (flowPhase === 'output' && e.to === 'END');
      return { 
        id: `e${i}-${layoutDirection}`,
        source: e.from, 
        target: e.to, 
        type: 'animated', 
        data: { animated },
        sourceHandle: isHorizontal ? 'right' : 'bottom',
        targetHandle: isHorizontal ? 'left' : 'top',
      };
    }));
  }, [project, flowPhase, activeAgent, setEdges, layoutDirection, isHorizontal]);

  return { nodes, edges, setNodes, setEdges, onNodesChange, onEdgesChange };
}
