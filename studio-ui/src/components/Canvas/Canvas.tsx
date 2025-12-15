import { useCallback, useEffect, useState, DragEvent } from 'react';
import {
  ReactFlow,
  Background,
  Controls,
  Node,
  Edge,
  useNodesState,
  useEdgesState,
  Connection,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { useStore } from '../../store';
import { TestConsole } from '../Console/TestConsole';

const AGENT_TYPES = [
  { type: 'llm', label: 'LLM Agent', enabled: true },
  { type: 'sequential', label: 'Sequential Agent', enabled: true },
  { type: 'loop', label: 'Loop Agent', enabled: true },
  { type: 'parallel', label: 'Parallel Agent', enabled: true },
];

const TOOL_TYPES = [
  { type: 'function', label: 'Function Tool', icon: 'ƒ' },
  { type: 'mcp', label: 'MCP Tool', icon: '🔌' },
  { type: 'browser', label: 'Browser Tool', icon: '🌐' },
  { type: 'exit_loop', label: 'Exit Loop', icon: '⏹' },
  { type: 'google_search', label: 'Google Search', icon: '🔍' },
  { type: 'load_artifact', label: 'Load Artifact', icon: '📦' },
];

type FlowPhase = 'idle' | 'input' | 'output';

export function Canvas() {
  const { currentProject, closeProject, saveProject, selectNode, selectedNodeId, updateAgent, addAgent, removeAgent, addEdge: addProjectEdge, removeEdge: removeProjectEdge, addToolToAgent, removeToolFromAgent, addSubAgentToContainer } = useStore();
  const [showConsole, setShowConsole] = useState(true);
  const [flowPhase, setFlowPhase] = useState<FlowPhase>('idle');
  const [selectedSubAgent, setSelectedSubAgent] = useState<{parent: string, sub: string} | null>(null);

  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);

  const removeSubAgent = useCallback((parentId: string, subId: string) => {
    if (!currentProject) return;
    const parent = currentProject.agents[parentId];
    if (!parent || parent.sub_agents.length <= 1) return;
    updateAgent(parentId, { sub_agents: parent.sub_agents.filter(s => s !== subId) });
    removeAgent(subId);
    setSelectedSubAgent(null);
  }, [currentProject, updateAgent, removeAgent]);

  useEffect(() => {
    if (!currentProject) return;
    
    const agentIds = Object.keys(currentProject.agents);
    // Filter out sub-agents (those that belong to a sequential)
    const allSubAgents = new Set(
      agentIds.flatMap(id => currentProject.agents[id].sub_agents || [])
    );
    const topLevelAgents = agentIds.filter(id => !allSubAgents.has(id));
    
    const newNodes: Node[] = [
      { id: 'START', position: { x: 200, y: 50 }, data: { label: '▶ START' }, type: 'input', style: { background: '#1a472a', border: '2px solid #4ade80', borderRadius: 8, padding: 10, color: '#fff' } },
      { id: 'END', position: { x: 200, y: 150 + topLevelAgents.length * 150 }, data: { label: '⏹ END' }, type: 'output', style: { background: '#4a1a1a', border: '2px solid #f87171', borderRadius: 8, padding: 10, color: '#fff' } },
    ];
    
    topLevelAgents.forEach((id, i) => {
      const agent = currentProject.agents[id];
      if (agent.type === 'sequential' || agent.type === 'loop' || agent.type === 'parallel') {
        const isParallel = agent.type === 'parallel';
        const isLoop = agent.type === 'loop';
        const subAgentLabels = (agent.sub_agents || []).map((subId, idx) => {
          const isSelected = selectedSubAgent?.parent === id && selectedSubAgent?.sub === subId;
          return (
            <div 
              key={subId} 
              className={`text-xs rounded px-2 py-1 cursor-pointer ${isParallel ? '' : 'mt-1'} ${isSelected ? 'bg-red-700 ring-1 ring-red-400' : 'bg-gray-700 hover:bg-gray-600'}`}
              onClick={(e) => { e.stopPropagation(); setSelectedSubAgent(isSelected ? null : {parent: id, sub: subId}); }}
            >
              {isParallel ? '' : `${idx + 1}. `}{subId}
              {isSelected && agent.sub_agents.length > 1 && (
                <button 
                  className="ml-2 text-red-300 hover:text-white"
                  onClick={(e) => { e.stopPropagation(); removeSubAgent(id, subId); }}
                >×</button>
              )}
            </div>
          );
        });
        const config = {
          sequential: { icon: '⛓', label: 'Sequential Agent', bg: '#1e3a5f', border: '#60a5fa' },
          loop: { icon: '🔄', label: `Loop Agent (${agent.max_iterations || 3}x)`, bg: '#3d1e5f', border: '#a855f7' },
          parallel: { icon: '⚡', label: 'Parallel Agent', bg: '#1e5f3d', border: '#34d399' },
        }[agent.type]!;
        newNodes.push({
          id,
          position: { x: 200, y: 150 + i * 150 },
          data: { 
            label: (
              <div className="text-center">
                <div className="font-semibold">{config.icon} {id}</div>
                <div className="text-xs text-gray-400 mb-1">{config.label}</div>
                <div className={`border-t border-gray-600 pt-1 mt-1 ${isLoop ? 'relative' : ''}`}>
                  {isLoop && (
                    <div className="absolute -left-2 top-0 bottom-0 w-1 border-l-2 border-t-2 border-b-2 border-purple-400 rounded-l" />
                  )}
                  {isParallel ? (
                    <div className="flex gap-1 flex-wrap justify-center">{subAgentLabels}</div>
                  ) : (
                    <div className={isLoop ? 'ml-1' : ''}>{subAgentLabels}</div>
                  )}
                  {isLoop && (
                    <div className="absolute -right-2 top-1/2 text-purple-400 text-xs">↩</div>
                  )}
                </div>
              </div>
            )
          },
          style: { background: config.bg, border: `2px solid ${config.border}`, borderRadius: 8, padding: 12, color: '#fff', minWidth: isParallel ? 250 : 150 },
        });
      } else {
        const tools = agent.tools || [];
        newNodes.push({
          id,
          position: { x: 200, y: 150 + i * 150 },
          data: { label: (
            <div className="text-center">
              <div>🤖 {id}</div>
              <div className="text-xs text-gray-400">LLM Agent</div>
              {tools.length > 0 && (
                <div className="border-t border-gray-600 pt-1 mt-1">
                  {tools.map(t => {
                    const tool = TOOL_TYPES.find(tt => tt.type === t);
                    return <div key={t} className="text-xs text-gray-300">{tool?.icon} {tool?.label || t}</div>;
                  })}
                </div>
              )}
            </div>
          )},
          style: { background: '#16213e', border: '2px solid #e94560', borderRadius: 8, padding: 12, color: '#fff', minWidth: 120 },
        });
      }
    });
    setNodes(newNodes);
  }, [currentProject, setNodes, selectedSubAgent, removeSubAgent]);

  // Update edges based on flow phase
  useEffect(() => {
    if (!currentProject) return;
    
    const newEdges: Edge[] = currentProject.workflow.edges.map((e, i) => {
      const isStartEdge = e.from === 'START';
      const isEndEdge = e.to === 'END';
      const animated = (flowPhase === 'input' && isStartEdge) || (flowPhase === 'output' && isEndEdge);
      
      return {
        id: `e${i}`,
        source: e.from,
        target: e.to,
        type: 'smoothstep',
        animated,
        style: { stroke: animated ? '#4ade80' : '#e94560', strokeWidth: 2 },
        interactionWidth: 20,
      };
    });
    setEdges(newEdges);
  }, [currentProject, flowPhase, setEdges]);

  const createAgent = useCallback((agentType: string = 'llm') => {
    if (!currentProject) return;
    const agentCount = Object.keys(currentProject.agents).length;
    const prefix = { sequential: 'seq', loop: 'loop', parallel: 'par' }[agentType] || 'agent';
    const id = `${prefix}_${agentCount + 1}`;
    
    if (agentType === 'sequential' || agentType === 'loop' || agentType === 'parallel') {
      // Create container with 2 default sub-agents
      const sub1 = `${id}_agent_1`;
      const sub2 = `${id}_agent_2`;
      addAgent(sub1, {
        type: 'llm',
        model: 'gemini-2.0-flash',
        instruction: 'You are agent 1.',
        tools: [],
        sub_agents: [],
        position: { x: 0, y: 0 },
      });
      addAgent(sub2, {
        type: 'llm',
        model: 'gemini-2.0-flash',
        instruction: 'You are agent 2.',
        tools: [],
        sub_agents: [],
        position: { x: 0, y: 0 },
      });
      addAgent(id, {
        type: agentType as 'sequential' | 'loop' | 'parallel',
        instruction: '',
        tools: [],
        sub_agents: [sub1, sub2],
        position: { x: 200, y: 150 + agentCount * 180 },
        max_iterations: agentType === 'loop' ? 3 : undefined,
      });
    } else {
      addAgent(id, {
        type: 'llm',
        model: 'gemini-2.0-flash',
        instruction: 'You are a helpful assistant.',
        tools: [],
        sub_agents: [],
        position: { x: 200, y: 150 + agentCount * 120 },
      });
    }
    addProjectEdge('START', id);
    addProjectEdge(id, 'END');
    selectNode(id);
  }, [currentProject, addAgent, addProjectEdge, selectNode]);

  const onDragStart = (e: DragEvent, nodeType: string) => {
    e.dataTransfer.setData('application/reactflow', nodeType);
    e.dataTransfer.effectAllowed = 'move';
  };

  const onDragOver = useCallback((e: DragEvent) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
  }, []);

  const onDrop = useCallback((e: DragEvent) => {
    e.preventDefault();
    const type = e.dataTransfer.getData('application/reactflow');
    if (!type) return;
    createAgent(type);
  }, [createAgent]);

  const onConnect = useCallback((params: Connection) => {
    if (params.source && params.target) {
      addProjectEdge(params.source, params.target);
    }
  }, [addProjectEdge]);

  const onEdgesDelete = useCallback((edgesToDelete: Edge[]) => {
    edgesToDelete.forEach((edge) => {
      removeProjectEdge(edge.source, edge.target);
    });
  }, [removeProjectEdge]);

  const onEdgeDoubleClick = useCallback((_: React.MouseEvent, edge: Edge) => {
    removeProjectEdge(edge.source, edge.target);
  }, [removeProjectEdge]);

  const onNodeClick = useCallback((_: React.MouseEvent, node: Node) => {
    if (node.id !== 'START' && node.id !== 'END') {
      selectNode(node.id);
    } else {
      selectNode(null);
    }
  }, [selectNode]);

  const onPaneClick = useCallback(() => selectNode(null), [selectNode]);

  if (!currentProject) return null;

  const selectedAgent = selectedNodeId ? currentProject.agents[selectedNodeId] : null;
  const hasAgents = Object.keys(currentProject.agents).length > 0;

  return (
    <div className="flex flex-col h-full">
      <div className="flex flex-1 overflow-hidden">
        {/* Palette */}
        <div className="w-48 bg-studio-panel border-r border-gray-700 p-4 flex flex-col overflow-y-auto">
          <h3 className="font-semibold mb-2">Agents</h3>
          <div className="space-y-1 mb-4">
            {AGENT_TYPES.map(({ type, label }) => (
              <div
                key={type}
                draggable
                onDragStart={(e) => onDragStart(e, type)}
                onClick={() => createAgent(type)}
                className="p-2 bg-studio-accent rounded text-sm cursor-grab hover:bg-studio-highlight"
              >
                ⊕ {label}
              </div>
            ))}
          </div>
          
          <h3 className="font-semibold mb-2">Tools</h3>
          <div className="space-y-1 flex-1">
            {TOOL_TYPES.map(({ type, label, icon }) => {
              const isAdded = selectedNodeId && currentProject?.agents[selectedNodeId]?.tools?.includes(type);
              return (
                <div
                  key={type}
                  className={`p-2 rounded text-sm cursor-pointer flex items-center gap-2 ${
                    isAdded ? 'bg-green-800 hover:bg-green-700' : 'bg-gray-700 hover:bg-gray-600'
                  } ${!selectedNodeId ? 'opacity-50' : ''}`}
                  onClick={() => {
                    if (!selectedNodeId) return;
                    if (isAdded) {
                      removeToolFromAgent(selectedNodeId, type);
                    } else {
                      addToolToAgent(selectedNodeId, type);
                    }
                  }}
                >
                  <span>{icon}</span>
                  <span className="text-xs">{label}</span>
                  {isAdded && <span className="ml-auto text-xs">✓</span>}
                </div>
              );
            })}
          </div>
          <div className="space-y-2">
            <button onClick={() => setShowConsole(!showConsole)} className="w-full px-3 py-2 bg-gray-700 rounded text-sm">
              {showConsole ? 'Hide Console' : 'Show Console'}
            </button>
            <button onClick={closeProject} className="w-full px-3 py-2 bg-gray-700 rounded text-sm">Back</button>
          </div>
        </div>

        {/* Canvas */}
        <div className="flex-1">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onEdgesDelete={onEdgesDelete}
            onEdgeDoubleClick={onEdgeDoubleClick}
            onConnect={onConnect}
            onNodeClick={onNodeClick}
            onPaneClick={onPaneClick}
            onDrop={onDrop}
            onDragOver={onDragOver}
            deleteKeyCode={['Backspace', 'Delete']}
            fitView
            fitViewOptions={{ padding: 0.3, maxZoom: 1 }}
            minZoom={0.1}
            maxZoom={2}
          >
            <Background color="#333" gap={20} />
            <Controls />
          </ReactFlow>
        </div>

        {/* Properties */}
        {selectedAgent && (
          <div className="w-72 bg-studio-panel border-l border-gray-700 p-4 overflow-y-auto">
            <div className="flex justify-between items-center mb-4">
              <h3 className="font-semibold">{selectedNodeId}</h3>
              <div className="flex gap-2">
                <button onClick={saveProject} className="px-2 py-1 bg-studio-highlight rounded text-xs">Save</button>
                <button onClick={() => selectNode(null)} className="px-2 py-1 bg-gray-600 rounded text-xs">Close</button>
              </div>
            </div>
            
            {(selectedAgent.type === 'sequential' || selectedAgent.type === 'loop' || selectedAgent.type === 'parallel') ? (
              /* Container Agent Properties */
              <div>
                {selectedAgent.type === 'loop' && (
                  <div className="mb-4">
                    <label className="block text-sm text-gray-400 mb-1">Max Iterations</label>
                    <input
                      type="number"
                      min="1"
                      className="w-full px-2 py-1 bg-studio-bg border border-gray-600 rounded text-sm"
                      value={selectedAgent.max_iterations || 3}
                      onChange={(e) => updateAgent(selectedNodeId!, { max_iterations: parseInt(e.target.value) || 3 })}
                    />
                  </div>
                )}
                <label className="block text-sm text-gray-400 mb-2">
                  Sub-Agents {selectedAgent.type === 'parallel' ? '(run concurrently)' : '(in order)'}
                </label>
                {(selectedAgent.sub_agents || []).map((subId, idx) => {
                  const subAgent = currentProject.agents[subId];
                  if (!subAgent) return null;
                  return (
                    <div key={subId} className="mb-4 p-2 bg-gray-800 rounded">
                      <div className="text-sm font-medium mb-2">{selectedAgent.type === 'parallel' ? '∥' : `${idx + 1}.`} {subId}</div>
                      <label className="block text-xs text-gray-400 mb-1">Model</label>
                      <input
                        className="w-full px-2 py-1 bg-studio-bg border border-gray-600 rounded text-xs mb-2"
                        value={subAgent.model || ''}
                        onChange={(e) => updateAgent(subId, { model: e.target.value })}
                      />
                      <label className="block text-xs text-gray-400 mb-1">Instruction</label>
                      <textarea
                        className="w-full px-2 py-1 bg-studio-bg border border-gray-600 rounded text-xs h-20"
                        value={subAgent.instruction}
                        onChange={(e) => updateAgent(subId, { instruction: e.target.value })}
                      />
                    </div>
                  );
                })}
                <button
                  onClick={() => addSubAgentToContainer(selectedNodeId!)}
                  className="w-full py-2 bg-gray-700 hover:bg-gray-600 rounded text-sm"
                >
                  + Add Sub-Agent
                </button>
              </div>
            ) : (
              /* LLM Agent Properties */
              <div>
                <label className="block text-sm text-gray-400 mb-1">Model</label>
                <input
                  className="w-full px-2 py-1 bg-studio-bg border border-gray-600 rounded text-sm mb-3"
                  value={selectedAgent.model || ''}
                  onChange={(e) => updateAgent(selectedNodeId!, { model: e.target.value })}
                />
                <label className="block text-sm text-gray-400 mb-1">Instruction</label>
                <textarea
                  className="w-full px-2 py-1 bg-studio-bg border border-gray-600 rounded text-sm h-24"
                  value={selectedAgent.instruction}
                  onChange={(e) => updateAgent(selectedNodeId!, { instruction: e.target.value })}
                />
                {selectedAgent.tools.length > 0 && (
                  <div className="mt-3">
                    <label className="block text-sm text-gray-400 mb-1">Tools</label>
                    <div className="flex flex-wrap gap-1">
                      {selectedAgent.tools.map(t => {
                        const tool = TOOL_TYPES.find(tt => tt.type === t);
                        return (
                          <span key={t} className="text-xs bg-gray-700 px-2 py-1 rounded flex items-center gap-1">
                            {tool?.icon} {tool?.label || t}
                            <button onClick={() => removeToolFromAgent(selectedNodeId!, t)} className="ml-1 text-red-400 hover:text-red-300">×</button>
                          </span>
                        );
                      })}
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
        )}
      </div>

      {/* Test Console */}
      {showConsole && hasAgents && (
        <div className="h-64">
          <TestConsole onFlowPhase={setFlowPhase} />
        </div>
      )}
      {showConsole && !hasAgents && (
        <div className="h-32 bg-studio-panel border-t border-gray-700 flex items-center justify-center text-gray-500">
          Drag "LLM Agent" onto the canvas to get started
        </div>
      )}
    </div>
  );
}
