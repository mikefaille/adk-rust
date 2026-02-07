import { useCallback, DragEvent } from 'react';
import { useStore } from '../store';
import type { ActionNodeType, ActionNodeConfig } from '../types/actionNodes';
import { createDefaultStandardProperties } from '../types/standardProperties';
import type { AutobuildTriggerType } from './useBuild';

/**
 * Parameters for the useCanvasDragDrop hook.
 */
export interface UseCanvasDragDropParams {
  /** Callback to create an agent with undo support */
  createAgentWithUndo: (agentType?: string) => void;
  /** Currently selected agent node ID */
  selectedNodeId: string | null;
  /** Callback to apply layout after node changes */
  applyLayout: () => void;
  /** Callback to invalidate the current build */
  invalidateBuild: (reason?: AutobuildTriggerType) => void;
}

/**
 * Return type for the useCanvasDragDrop hook.
 */
export interface UseCanvasDragDropReturn {
  /** Handler for agent palette drag start */
  onDragStart: (e: DragEvent, type: string) => void;
  /** Handler for action node palette drag start */
  onActionDragStart: (e: DragEvent, type: ActionNodeType) => void;
  /** Handler for drag over the canvas */
  onDragOver: (e: DragEvent) => void;
  /** Handler for dropping items on the canvas */
  onDrop: (e: DragEvent) => void;
  /** Create an action node and wire it into the workflow */
  createActionNode: (type: ActionNodeType) => void;
}

/**
 * Hook that encapsulates all drag-and-drop handlers for the Canvas.
 *
 * Handles:
 * - Agent palette drag start
 * - Action node palette drag start
 * - Canvas drag over
 * - Canvas drop (agents, action nodes, tools)
 * - Action node creation with workflow edge wiring
 *
 * @see Requirements 2.5
 */
export function useCanvasDragDrop({
  createAgentWithUndo,
  selectedNodeId,
  applyLayout,
  invalidateBuild,
}: UseCanvasDragDropParams): UseCanvasDragDropReturn {
  const addActionNode = useStore(s => s.addActionNode);
  const addProjectEdge = useStore(s => s.addEdge);
  const removeProjectEdge = useStore(s => s.removeEdge);
  const selectActionNode = useStore(s => s.selectActionNode);
  const addToolToAgent = useStore(s => s.addToolToAgent);

  // Agent palette drag start handler
  const onDragStart = (e: DragEvent, type: string) => {
    e.dataTransfer.setData('application/reactflow', type);
    e.dataTransfer.effectAllowed = 'move';
  };

  // Action node palette drag start handler
  const onActionDragStart = (e: DragEvent, type: ActionNodeType) => {
    e.dataTransfer.setData('application/actionnode', type);
    e.dataTransfer.effectAllowed = 'move';
  };

  // Create action node handler
  // Action nodes integrate into the workflow the same way agents do:
  // - If first item on canvas, connect START -> node -> END
  // - If other items exist, insert before END (remove edge to END, connect previous -> new -> END)
  const createActionNode = useCallback((type: ActionNodeType) => {
    // Read current project from store directly to avoid stale closures
    const currentProject = useStore.getState().currentProject;
    if (!currentProject) return;

    const id = `${type}_${Date.now()}`;
    const name = type.charAt(0).toUpperCase() + type.slice(1);
    const baseProps = createDefaultStandardProperties(id, name, `${type}Result`);

    // Create node config based on type
    let nodeConfig: ActionNodeConfig;

    switch (type) {
      case 'trigger':
        nodeConfig = { ...baseProps, type: 'trigger', triggerType: 'manual' };
        break;
      case 'http':
        nodeConfig = {
          ...baseProps,
          type: 'http',
          method: 'GET',
          url: 'https://api.example.com',
          auth: { type: 'none' },
          headers: {},
          body: { type: 'none' },
          response: { type: 'json' },
        };
        break;
      case 'set':
        nodeConfig = { ...baseProps, type: 'set', mode: 'set', variables: [] };
        break;
      case 'transform':
        nodeConfig = { ...baseProps, type: 'transform', transformType: 'jsonpath', expression: '' };
        break;
      case 'switch':
        nodeConfig = { ...baseProps, type: 'switch', evaluationMode: 'first_match', conditions: [] };
        break;
      case 'loop':
        nodeConfig = {
          ...baseProps,
          type: 'loop',
          loopType: 'forEach',
          forEach: { sourceArray: '', itemVar: 'item', indexVar: 'index' },
          parallel: { enabled: false },
          results: { collect: true },
        };
        break;
      case 'merge':
        nodeConfig = {
          ...baseProps,
          type: 'merge',
          mode: 'wait_all',
          combineStrategy: 'array',
          timeout: { enabled: false, ms: 30000, behavior: 'error' },
        };
        break;
      case 'wait':
        nodeConfig = {
          ...baseProps,
          type: 'wait',
          waitType: 'fixed',
          fixed: { duration: 1000, unit: 'ms' },
        };
        break;
      case 'code':
        nodeConfig = {
          ...baseProps,
          type: 'code',
          language: 'javascript',
          code: '// Your code here\nreturn input;',
          sandbox: { networkAccess: false, fileSystemAccess: false, memoryLimit: 128, timeLimit: 5000 },
        };
        break;
      case 'database':
        nodeConfig = {
          ...baseProps,
          type: 'database',
          dbType: 'postgresql',
          connection: { connectionString: '' },
        };
        break;
      case 'email':
        nodeConfig = {
          ...baseProps,
          type: 'email',
          mode: 'send',
          smtp: {
            host: 'smtp.example.com',
            port: 587,
            secure: true,
            username: '',
            password: '',
            fromEmail: '',
          },
          recipients: { to: '' },
          content: { subject: '', body: '', bodyType: 'text' },
          attachments: [],
        };
        break;
      default:
        return;
    }

    // Add the action node
    addActionNode(id, nodeConfig);

    // Special handling for trigger nodes:
    // - Only one trigger allowed per workflow
    // - Trigger connects TO START (not from START like other nodes)
    // - Visual flow: [Trigger] → START → agents → END
    if (type === 'trigger') {
      // Check if a trigger already exists
      const existingTrigger = Object.values(currentProject.actionNodes || {}).find(
        (node) => node.type === 'trigger'
      );
      if (existingTrigger && existingTrigger.id !== id) {
        // Remove the newly added trigger - only one allowed
        useStore.getState().removeActionNode(id);
        alert('Only one trigger node is allowed per workflow. Remove the existing trigger first.');
        return;
      }

      // Connect trigger TO START (trigger is the entry point)
      addProjectEdge(id, 'START');
    } else {
      // Connect to workflow edges (same logic as agents)
      // Find edge going to END and insert this node before it
      const edgeToEnd = currentProject.workflow.edges.find(e => e.to === 'END');
      if (edgeToEnd) {
        // Remove the existing edge to END
        removeProjectEdge(edgeToEnd.from, 'END');
        // Connect previous node to this new node
        addProjectEdge(edgeToEnd.from, id);
      } else {
        // No existing edges to END, connect from START
        addProjectEdge('START', id);
      }
      // Connect this node to END
      addProjectEdge(id, 'END');
    }

    selectActionNode(id);
    invalidateBuild('onAgentAdd'); // Action nodes use same trigger as agents
    setTimeout(() => applyLayout(), 100);
  }, [addActionNode, addProjectEdge, removeProjectEdge, selectActionNode, applyLayout, invalidateBuild]);

  const onDragOver = useCallback((e: DragEvent) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = e.dataTransfer.types.includes('text/plain') ? 'copy' : 'move';
  }, []);

  const onDrop = useCallback((e: DragEvent) => {
    e.preventDefault();

    // Read current project from store directly to avoid stale closures
    const currentProject = useStore.getState().currentProject;

    const toolData = e.dataTransfer.getData('text/plain');
    if (toolData.startsWith('tool:') && selectedNodeId && currentProject?.agents[selectedNodeId]) {
      addToolToAgent(selectedNodeId, toolData.slice(5));
      invalidateBuild('onToolAdd'); // Trigger autobuild when tool is added
      return;
    }

    // Handle action node drop
    const actionType = e.dataTransfer.getData('application/actionnode');
    if (actionType) {
      createActionNode(actionType as ActionNodeType);
      return;
    }

    const type = e.dataTransfer.getData('application/reactflow');
    if (type) {
      createAgentWithUndo(type);
      invalidateBuild('onAgentAdd'); // Trigger autobuild when agent is added
      // Apply layout after adding node (only in fixed mode or always for initial setup)
      setTimeout(() => applyLayout(), 100);
    }
  }, [createAgentWithUndo, createActionNode, selectedNodeId, addToolToAgent, applyLayout, invalidateBuild]);

  return {
    onDragStart,
    onActionDragStart,
    onDragOver,
    onDrop,
    createActionNode,
  };
}
