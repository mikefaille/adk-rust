import { create } from 'zustand';
import type { Project, ProjectMeta, AgentSchema, ToolConfig, Edge } from '../types/project';
import type { ActionNodeConfig } from '../types/actionNodes';
import type { LayoutDirection, LayoutMode } from '../types/layout';
import { api } from '../api/client';
import { loadGlobalSettings } from '../types/settings';

interface StudioState {
  // Project list
  projects: ProjectMeta[];
  loadingProjects: boolean;
  
  // Current project
  currentProject: Project | null;
  selectedNodeId: string | null;
  selectedToolId: string | null;
  
  // Action node selection
  selectedActionNodeId: string | null;
  
  // Layout state
  layoutMode: LayoutMode;
  layoutDirection: LayoutDirection;
  snapToGrid: boolean;
  gridSize: number;
  
  // Data flow overlay state (v2.0)
  showDataFlowOverlay: boolean;
  
  // Debug mode state (v2.0) - controls visibility of StateInspector and Timeline
  debugMode: boolean;
  
  // Actions
  fetchProjects: () => Promise<void>;
  createProject: (name: string, description?: string) => Promise<Project>;
  openProject: (id: string) => Promise<void>;
  saveProject: () => Promise<void>;
  closeProject: () => void;
  deleteProject: (id: string) => Promise<void>;
  updateProjectMeta: (name: string, description: string) => void;
  updateProjectSettings: (settings: Partial<import('../types/project').ProjectSettings>) => void;
  
  // Canvas actions
  selectNode: (id: string | null) => void;
  updateAgent: (id: string, updates: Partial<AgentSchema>) => void;
  renameAgent: (oldId: string, newId: string) => void;
  addAgent: (id: string, agent: AgentSchema) => void;
  removeAgent: (id: string) => void;
  addEdge: (from: string, to: string, fromPort?: string, toPort?: string) => void;
  removeEdge: (from: string, to: string) => void;
  setEdges: (edges: Edge[]) => void;
  addToolToAgent: (agentId: string, toolType: string) => void;
  removeToolFromAgent: (agentId: string, toolType: string) => void;
  addSubAgentToContainer: (containerId: string) => void;
  
  // Tool config actions
  selectTool: (toolId: string | null) => void;
  updateToolConfig: (toolId: string, config: ToolConfig) => void;
  
  // Action node actions (v2.0)
  selectActionNode: (id: string | null) => void;
  addActionNode: (id: string, node: ActionNodeConfig) => void;
  updateActionNode: (id: string, updates: Partial<ActionNodeConfig>) => void;
  removeActionNode: (id: string) => void;
  renameActionNode: (oldId: string, newId: string) => void;
  
  // Layout actions
  setLayoutMode: (mode: LayoutMode) => void;
  setLayoutDirection: (dir: LayoutDirection) => void;
  setSnapToGrid: (snap: boolean) => void;
  setGridSize: (size: number) => void;
  
  // Data flow overlay actions (v2.0)
  setShowDataFlowOverlay: (show: boolean) => void;
  
  // Debug mode actions (v2.0)
  setDebugMode: (enabled: boolean) => void;
}

export const useStore = create<StudioState>((set, get) => ({
  projects: [],
  loadingProjects: false,
  currentProject: null,
  selectedNodeId: null,
  selectedToolId: null,
  
  // Action node selection
  selectedActionNodeId: null,
  
  // Layout state with defaults
  layoutMode: 'free',
  layoutDirection: 'LR',  // Default to horizontal (left-to-right) layout
  snapToGrid: true,
  gridSize: 20,
  
  // Data flow overlay state (v2.0)
  showDataFlowOverlay: false,
  
  // Debug mode state (v2.0) - controls visibility of StateInspector and Timeline
  debugMode: false,

  fetchProjects: async () => {
    set({ loadingProjects: true });
    try {
      const projects = await api.projects.list();
      set({ projects });
    } finally {
      set({ loadingProjects: false });
    }
  },

  createProject: async (name, description) => {
    const project = await api.projects.create(name, description);
    set((s) => ({ projects: [{ id: project.id, name, description: description || '', updated_at: project.updated_at }, ...s.projects] }));
    return project;
  },

  openProject: async (id) => {
    const project = await api.projects.get(id);
    const globalSettings = loadGlobalSettings();
    // Restore layout settings from project if available, otherwise use global defaults
    const layoutMode = project.settings?.layoutMode || globalSettings.layoutMode;
    const layoutDirection = project.settings?.layoutDirection || globalSettings.layoutDirection;
    const showDataFlowOverlay = project.settings?.showDataFlowOverlay ?? globalSettings.showDataFlowOverlay;
    const debugMode = project.settings?.debugMode ?? false;
    set({ 
      currentProject: project, 
      selectedNodeId: null,
      layoutMode,
      layoutDirection,
      showDataFlowOverlay,
      debugMode,
    });
  },

  saveProject: async () => {
    const { currentProject, layoutMode, layoutDirection, showDataFlowOverlay, debugMode } = get();
    if (!currentProject) return;
    // Include layout settings and data flow overlay preference in project before saving
    const projectToSave = {
      ...currentProject,
      settings: {
        ...currentProject.settings,
        layoutMode,
        layoutDirection,
        showDataFlowOverlay,
        debugMode,
      },
    };
    await api.projects.update(currentProject.id, projectToSave);
  },

  closeProject: () => set({ currentProject: null, selectedNodeId: null, selectedActionNodeId: null }),

  deleteProject: async (id) => {
    await api.projects.delete(id);
    set((s) => ({ projects: s.projects.filter((p) => p.id !== id) }));
  },

  updateProjectMeta: (name, description) => {
    set((s) => {
      if (!s.currentProject) return s;
      return {
        currentProject: { ...s.currentProject, name, description },
        projects: s.projects.map(p => 
          p.id === s.currentProject?.id 
            ? { ...p, name, description } 
            : p
        ),
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  updateProjectSettings: (settings) => {
    set((s) => {
      if (!s.currentProject) return s;
      const newSettings = { ...s.currentProject.settings, ...settings };
      // Also update local layout state if layout settings changed
      const updates: Partial<StudioState> = {
        currentProject: { ...s.currentProject, settings: newSettings },
      };
      if (settings.layoutMode !== undefined) updates.layoutMode = settings.layoutMode;
      if (settings.layoutDirection !== undefined) updates.layoutDirection = settings.layoutDirection;
      if (settings.showDataFlowOverlay !== undefined) updates.showDataFlowOverlay = settings.showDataFlowOverlay;
      return updates;
    });
    setTimeout(() => get().saveProject(), 0);
  },

  selectNode: (id) => set({ selectedNodeId: id }),

  updateAgent: (id, updates) =>
    set((s) => {
      if (!s.currentProject) return s;
      return {
        currentProject: {
          ...s.currentProject,
          agents: {
            ...s.currentProject.agents,
            [id]: { ...s.currentProject.agents[id], ...updates },
          },
        },
      };
    }),

  renameAgent: (oldId, newId) => {
    if (oldId === newId) return;
    set((s) => {
      if (!s.currentProject || !s.currentProject.agents[oldId]) return s;
      
      // Clone agents, add new key, remove old
      const agents = { ...s.currentProject.agents };
      agents[newId] = agents[oldId];
      delete agents[oldId];
      
      // Update sub_agents references in containers
      Object.keys(agents).forEach(id => {
        if (agents[id].sub_agents?.includes(oldId)) {
          agents[id] = { ...agents[id], sub_agents: agents[id].sub_agents.map(s => s === oldId ? newId : s) };
        }
      });
      
      // Update edges
      const edges = s.currentProject.workflow.edges.map(e => ({
        ...e,
        from: e.from === oldId ? newId : e.from,
        to: e.to === oldId ? newId : e.to,
      }));
      
      // Update tool configs
      const toolConfigs = { ...s.currentProject.tool_configs };
      Object.keys(toolConfigs).forEach(key => {
        if (key.startsWith(`${oldId}_`)) {
          const newKey = key.replace(`${oldId}_`, `${newId}_`);
          toolConfigs[newKey] = toolConfigs[key];
          delete toolConfigs[key];
        }
      });
      
      return {
        currentProject: { ...s.currentProject, agents, tool_configs: toolConfigs, workflow: { ...s.currentProject.workflow, edges } },
        selectedNodeId: s.selectedNodeId === oldId ? newId : s.selectedNodeId,
      };
    });
    get().saveProject();
  },

  addAgent: (id, agent) => {
    set((s) => {
      if (!s.currentProject) return s;
      return {
        currentProject: {
          ...s.currentProject,
          agents: { ...s.currentProject.agents, [id]: agent },
        },
      };
    });
    // Auto-save after state update
    setTimeout(() => get().saveProject(), 0);
  },

  removeAgent: (id) => {
    set((s) => {
      if (!s.currentProject) return s;
      const agent = s.currentProject.agents[id];
      
      // Collect all agents to remove (including sub-agents for containers)
      const agentsToRemove = [id];
      if (agent?.sub_agents) {
        agentsToRemove.push(...agent.sub_agents);
      }
      
      // Remove all agents
      const agents = { ...s.currentProject.agents };
      agentsToRemove.forEach(agentId => delete agents[agentId]);
      
      // Remove tool configs for all removed agents
      const toolConfigs = { ...s.currentProject.tool_configs };
      Object.keys(toolConfigs).forEach(key => {
        if (agentsToRemove.some(agentId => key.startsWith(`${agentId}_`))) {
          delete toolConfigs[key];
        }
      });
      
      // Get all action nodes
      const actionNodes = s.currentProject.actionNodes || {};
      
      // Reconnect edges: connect sources to targets to maintain flow
      const currentEdges = s.currentProject.workflow.edges;
      const newEdges: typeof currentEdges = [];
      
      // For each agent being removed, find incoming and outgoing edges
      for (const removedId of agentsToRemove) {
        const incomingEdges = currentEdges.filter(e => e.to === removedId);
        const outgoingEdges = currentEdges.filter(e => e.from === removedId);
        
        // Connect each source to each target
        for (const incoming of incomingEdges) {
          for (const outgoing of outgoingEdges) {
            // Don't create self-loops or duplicate edges
            if (incoming.from !== outgoing.to) {
              const edgeExists = newEdges.some(e => e.from === incoming.from && e.to === outgoing.to) ||
                                 currentEdges.some(e => e.from === incoming.from && e.to === outgoing.to && !agentsToRemove.includes(e.from) && !agentsToRemove.includes(e.to));
              if (!edgeExists) {
                newEdges.push({ from: incoming.from, to: outgoing.to });
              }
            }
          }
        }
      }
      
      // Keep edges not involving removed agents, plus add reconnected edges
      const remainingEdges = currentEdges.filter((e) => 
        !agentsToRemove.includes(e.from) && !agentsToRemove.includes(e.to)
      );
      
      let finalEdges = [...remainingEdges, ...newEdges];
      
      // Validate and fix edge connections
      // Get all remaining workflow nodes (agents + action nodes)
      const remainingAgentIds = Object.keys(agents);
      const actionNodeIds = Object.keys(actionNodes);
      const allNodeIds = [...remainingAgentIds, ...actionNodeIds];
      
      // Check each node has incoming edge (except START)
      for (const nodeId of allNodeIds) {
        const hasIncoming = finalEdges.some(e => e.to === nodeId);
        if (!hasIncoming) {
          // Find what should connect to this node
          // If there's an edge from START that doesn't go anywhere valid, connect START to this node
          const startEdge = finalEdges.find(e => e.from === 'START');
          if (!startEdge || !allNodeIds.includes(startEdge.to)) {
            // Remove invalid START edge if exists
            finalEdges = finalEdges.filter(e => !(e.from === 'START' && !allNodeIds.includes(e.to)));
            // Connect START to this node
            finalEdges.push({ from: 'START', to: nodeId });
          }
        }
      }
      
      // Check each node has outgoing edge (except END)
      for (const nodeId of allNodeIds) {
        const hasOutgoing = finalEdges.some(e => e.from === nodeId);
        if (!hasOutgoing) {
          // Connect this node to END
          finalEdges.push({ from: nodeId, to: 'END' });
        }
      }
      
      // Ensure START has an outgoing edge if there are nodes
      if (allNodeIds.length > 0) {
        const startHasOutgoing = finalEdges.some(e => e.from === 'START');
        if (!startHasOutgoing) {
          // Find the first node in the workflow
          const firstNode = allNodeIds[0];
          finalEdges.push({ from: 'START', to: firstNode });
        }
      }
      
      // Ensure END has an incoming edge if there are nodes
      if (allNodeIds.length > 0) {
        const endHasIncoming = finalEdges.some(e => e.to === 'END');
        if (!endHasIncoming) {
          // Find the last node in the workflow
          const lastNode = allNodeIds[allNodeIds.length - 1];
          finalEdges.push({ from: lastNode, to: 'END' });
        }
      }
      
      return {
        currentProject: {
          ...s.currentProject,
          agents,
          tool_configs: toolConfigs,
          workflow: {
            ...s.currentProject.workflow,
            edges: finalEdges,
          },
        },
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  addEdge: (from, to, fromPort, toPort) => {
    set((s) => {
      if (!s.currentProject) return s;
      const edge: Edge = { from, to };
      if (fromPort) edge.fromPort = fromPort;
      if (toPort) edge.toPort = toPort;
      return {
        currentProject: {
          ...s.currentProject,
          workflow: {
            ...s.currentProject.workflow,
            edges: [...s.currentProject.workflow.edges, edge],
          },
        },
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  removeEdge: (from, to) => {
    set((s) => {
      if (!s.currentProject) return s;
      return {
        currentProject: {
          ...s.currentProject,
          workflow: {
            ...s.currentProject.workflow,
            edges: s.currentProject.workflow.edges.filter((e) => !(e.from === from && e.to === to)),
          },
        },
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  setEdges: (edges) => {
    set((s) => {
      if (!s.currentProject) return s;
      return {
        currentProject: {
          ...s.currentProject,
          workflow: {
            ...s.currentProject.workflow,
            edges,
          },
        },
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  addToolToAgent: (agentId, toolType) => {
    set((s) => {
      if (!s.currentProject) return s;
      const agent = s.currentProject.agents[agentId];
      if (!agent) return s;
      
      // For function and mcp tools, generate unique ID to allow multiple
      let toolId = toolType;
      if (toolType === 'function' || toolType === 'mcp') {
        const existing = agent.tools.filter(t => t.startsWith(toolType));
        toolId = `${toolType}_${existing.length + 1}`;
      } else if (agent.tools.includes(toolType)) {
        return s; // Other tools can only be added once
      }
      
      return {
        currentProject: {
          ...s.currentProject,
          agents: {
            ...s.currentProject.agents,
            [agentId]: { ...agent, tools: [...agent.tools, toolId] },
          },
        },
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  removeToolFromAgent: (agentId, toolType) => {
    set((s) => {
      if (!s.currentProject) return s;
      const agent = s.currentProject.agents[agentId];
      if (!agent) return s;
      const toolConfigId = `${agentId}_${toolType}`;
      const { [toolConfigId]: _, ...remainingConfigs } = s.currentProject.tool_configs;
      return {
        currentProject: {
          ...s.currentProject,
          agents: {
            ...s.currentProject.agents,
            [agentId]: { ...agent, tools: agent.tools.filter(t => t !== toolType) },
          },
          tool_configs: remainingConfigs,
        },
        selectedToolId: s.selectedToolId === toolConfigId ? null : s.selectedToolId,
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  addSubAgentToContainer: (containerId) => {
    const { currentProject, addAgent, updateAgent, saveProject } = get();
    if (!currentProject) return;
    const container = currentProject.agents[containerId];
    if (!container) return;
    const subCount = container.sub_agents.length + 1;
    const newId = `${containerId}_agent_${subCount}`;
    addAgent(newId, {
      type: 'llm',
      model: 'gemini-2.0-flash',
      instruction: `You are agent ${subCount}.`,
      tools: [],
      sub_agents: [],
      position: { x: 0, y: 0 },
    });
    updateAgent(containerId, { sub_agents: [...container.sub_agents, newId] });
    saveProject();
  },

  selectTool: (toolId) => set({ selectedToolId: toolId }),

  updateToolConfig: (toolId, config) => {
    set((s) => {
      if (!s.currentProject) return s;
      return {
        currentProject: {
          ...s.currentProject,
          tool_configs: { ...s.currentProject.tool_configs, [toolId]: config },
        },
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  // Action node actions (v2.0)
  // @see Requirement 12.3: Canvas integration with action nodes
  
  selectActionNode: (id) => set((s) => ({ 
    selectedActionNodeId: id, 
    // Only clear selectedNodeId when selecting an action node (id is not null)
    selectedNodeId: id ? null : s.selectedNodeId, 
    selectedToolId: id ? null : s.selectedToolId,
  })),

  addActionNode: (id, node) => {
    set((s) => {
      if (!s.currentProject) return s;
      // Ensure actionNodes exists (for backward compatibility)
      const actionNodes = s.currentProject.actionNodes || {};
      return {
        currentProject: {
          ...s.currentProject,
          actionNodes: { ...actionNodes, [id]: node },
        },
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  updateActionNode: (id, updates) => {
    set((s) => {
      if (!s.currentProject) return s;
      const actionNodes = s.currentProject.actionNodes || {};
      if (!actionNodes[id]) return s;
      return {
        currentProject: {
          ...s.currentProject,
          actionNodes: {
            ...actionNodes,
            [id]: { ...actionNodes[id], ...updates } as ActionNodeConfig,
          },
        },
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  removeActionNode: (id) => {
    set((s) => {
      if (!s.currentProject) return s;
      const actionNodes = { ...(s.currentProject.actionNodes || {}) };
      delete actionNodes[id];
      
      // Reconnect edges: connect sources to targets to maintain flow
      const currentEdges = s.currentProject.workflow.edges;
      const newEdges: typeof currentEdges = [];
      
      // Find incoming and outgoing edges for the removed node
      const incomingEdges = currentEdges.filter(e => e.to === id);
      const outgoingEdges = currentEdges.filter(e => e.from === id);
      
      // Connect each source to each target
      for (const incoming of incomingEdges) {
        for (const outgoing of outgoingEdges) {
          // Don't create self-loops or duplicate edges
          if (incoming.from !== outgoing.to) {
            const edgeExists = newEdges.some(e => e.from === incoming.from && e.to === outgoing.to) ||
                               currentEdges.some(e => e.from === incoming.from && e.to === outgoing.to && e.from !== id && e.to !== id);
            if (!edgeExists) {
              newEdges.push({ from: incoming.from, to: outgoing.to });
            }
          }
        }
      }
      
      // Keep edges not involving removed node, plus add reconnected edges
      const remainingEdges = currentEdges.filter(
        (e) => e.from !== id && e.to !== id
      );
      
      let finalEdges = [...remainingEdges, ...newEdges];
      
      // Validate and fix edge connections
      const agents = s.currentProject.agents;
      const remainingAgentIds = Object.keys(agents);
      const actionNodeIds = Object.keys(actionNodes);
      const allNodeIds = [...remainingAgentIds, ...actionNodeIds];
      
      // Check each node has incoming edge (except START)
      for (const nodeId of allNodeIds) {
        const hasIncoming = finalEdges.some(e => e.to === nodeId);
        if (!hasIncoming) {
          const startEdge = finalEdges.find(e => e.from === 'START');
          if (!startEdge || !allNodeIds.includes(startEdge.to)) {
            finalEdges = finalEdges.filter(e => !(e.from === 'START' && !allNodeIds.includes(e.to)));
            finalEdges.push({ from: 'START', to: nodeId });
          }
        }
      }
      
      // Check each node has outgoing edge (except END)
      for (const nodeId of allNodeIds) {
        const hasOutgoing = finalEdges.some(e => e.from === nodeId);
        if (!hasOutgoing) {
          finalEdges.push({ from: nodeId, to: 'END' });
        }
      }
      
      // Ensure START has an outgoing edge if there are nodes
      if (allNodeIds.length > 0) {
        const startHasOutgoing = finalEdges.some(e => e.from === 'START');
        if (!startHasOutgoing) {
          finalEdges.push({ from: 'START', to: allNodeIds[0] });
        }
      }
      
      // Ensure END has an incoming edge if there are nodes
      if (allNodeIds.length > 0) {
        const endHasIncoming = finalEdges.some(e => e.to === 'END');
        if (!endHasIncoming) {
          finalEdges.push({ from: allNodeIds[allNodeIds.length - 1], to: 'END' });
        }
      }
      
      return {
        currentProject: {
          ...s.currentProject,
          actionNodes,
          workflow: {
            ...s.currentProject.workflow,
            edges: finalEdges,
          },
        },
        selectedActionNodeId: s.selectedActionNodeId === id ? null : s.selectedActionNodeId,
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  renameActionNode: (oldId, newId) => {
    if (oldId === newId) return;
    set((s) => {
      if (!s.currentProject) return s;
      const actionNodes = s.currentProject.actionNodes || {};
      if (!actionNodes[oldId]) return s;
      
      // Clone action nodes, add new key, remove old
      const newActionNodes = { ...actionNodes };
      newActionNodes[newId] = { ...newActionNodes[oldId], id: newId };
      delete newActionNodes[oldId];
      
      // Update edges
      const edges = s.currentProject.workflow.edges.map(e => ({
        ...e,
        from: e.from === oldId ? newId : e.from,
        to: e.to === oldId ? newId : e.to,
      }));
      
      return {
        currentProject: {
          ...s.currentProject,
          actionNodes: newActionNodes,
          workflow: { ...s.currentProject.workflow, edges },
        },
        selectedActionNodeId: s.selectedActionNodeId === oldId ? newId : s.selectedActionNodeId,
      };
    });
    setTimeout(() => get().saveProject(), 0);
  },

  setLayoutMode: (mode) => set({ layoutMode: mode }),

  setLayoutDirection: (dir) => set({ layoutDirection: dir }),

  setSnapToGrid: (snap) => set({ snapToGrid: snap }),

  setGridSize: (size) => set({ gridSize: size }),

  // Data flow overlay actions (v2.0)
  // @see Requirements 3.4: Store preference in project settings
  setShowDataFlowOverlay: (show) => {
    set({ showDataFlowOverlay: show });
    // Auto-save after state update
    setTimeout(() => get().saveProject(), 0);
  },
  
  // Debug mode actions (v2.0)
  // Controls visibility of StateInspector and Timeline
  setDebugMode: (enabled) => {
    set({ debugMode: enabled });
    // Auto-save after state update
    setTimeout(() => get().saveProject(), 0);
  },
}));
