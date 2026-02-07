import { useCallback, useState, useMemo } from 'react';
import type { StateSnapshot, InterruptData } from '../types/execution';
import { useExecutionPath } from './useExecutionPath';

/**
 * Flow phase for edge animations.
 * - 'idle': No activity
 * - 'trigger_input': User submitting input to trigger (animates trigger→START)
 * - 'input': Data flowing from START to agents
 * - 'output': Agent generating response
 * - 'interrupted': Waiting for HITL response
 * @see trigger-input-flow Requirements 2.2, 2.3, 3.1, 3.2
 */
export type FlowPhase = 'idle' | 'trigger_input' | 'input' | 'output' | 'interrupted';

/**
 * Hook that manages all execution-related state for the Canvas:
 * - Flow phase, active agent, iteration, thoughts
 * - Execution path tracking
 * - Timeline & state inspector (snapshots, scrubbing)
 * - Data flow overlay (state keys, highlighting)
 * - HITL interrupt state
 *
 * @see Requirements 2.1, 2.4: Canvas delegates execution state management
 */
export function useCanvasExecution(deps: {
  showDataFlowOverlay: boolean;
  setShowDataFlowOverlay: (v: boolean) => void;
}) {
  const { showDataFlowOverlay, setShowDataFlowOverlay } = deps;

  // Execution state
  const [flowPhase, setFlowPhase] = useState<FlowPhase>('idle');
  const [activeAgent, setActiveAgent] = useState<string | null>(null);
  const [iteration, setIteration] = useState(0);
  const [thoughts, setThoughts] = useState<Record<string, string>>({});

  // Execution path tracking (v2.0)
  // @see Requirements 10.3, 10.5: Execution path highlighting
  const executionPath = useExecutionPath();

  // Timeline state (v2.0)
  const [timelineCollapsed, setTimelineCollapsed] = useState(false);
  const [snapshots, setSnapshots] = useState<StateSnapshot[]>([]);
  const [currentSnapshotIndex, setCurrentSnapshotIndex] = useState(-1);
  const [scrubToFn, setScrubToFn] = useState<((index: number) => void) | null>(null);

  // State Inspector visibility (v2.0)
  const [showStateInspector, setShowStateInspector] = useState(true);

  // HITL: Interrupted node ID for visual indicator (v2.0)
  // @see trigger-input-flow Requirement 3.3: Interrupt visual indicator
  const [interruptedNodeId, setInterruptedNodeId] = useState<string | null>(null);

  // Data Flow Overlay state (v2.0)
  // @see Requirements 3.1-3.9: Data flow overlays
  const [stateKeys, setStateKeys] = useState<Map<string, string[]>>(new Map());
  const [highlightedKey, setHighlightedKey] = useState<string | null>(null);

  // v2.0: Wrapper for flow phase that also updates execution path
  // @see Requirements 10.3, 10.5: Execution path highlighting
  const handleFlowPhase = useCallback((phase: FlowPhase) => {
    setFlowPhase(phase);
    if (phase === 'input') {
      executionPath.startExecution();
    } else if (phase === 'idle' && executionPath.isExecuting) {
      executionPath.completeExecution();
    }
  }, [executionPath]);

  // v2.0: Wrapper for active agent that also updates execution path
  const handleActiveAgent = useCallback((agent: string | null) => {
    setActiveAgent(agent);
    if (agent && executionPath.isExecuting && !executionPath.path.includes(agent)) {
      executionPath.moveToNode(agent);
    }
  }, [executionPath]);

  // Thought bubble handler
  const handleThought = useCallback((agent: string, thought: string | null) => {
    setThoughts(prev =>
      thought
        ? { ...prev, [agent]: thought }
        : Object.fromEntries(Object.entries(prev).filter(([k]) => k !== agent))
    );
  }, []);

  // Handler for state key hover (for highlighting related edges)
  // @see Requirements 3.8: Highlight all edges using same key on hover
  const handleKeyHover = useCallback((key: string | null) => {
    setHighlightedKey(key);
  }, []);

  // Handler for toggling data flow overlay
  // @see Requirements 3.4: Toggle to show/hide data flow overlays
  const handleToggleDataFlowOverlay = useCallback(() => {
    setShowDataFlowOverlay(!showDataFlowOverlay);
  }, [showDataFlowOverlay, setShowDataFlowOverlay]);

  // HITL: Handler for interrupt state changes from TestConsole
  // @see trigger-input-flow Requirement 3.3: Interrupt visual indicator
  const handleInterruptChange = useCallback((interrupt: InterruptData | null) => {
    setInterruptedNodeId(interrupt?.nodeId || null);
  }, []);

  // Handler for receiving snapshots and state keys from TestConsole
  const handleSnapshotsChange = useCallback((
    newSnapshots: StateSnapshot[],
    newIndex: number,
    scrubTo: (index: number) => void,
    newStateKeys?: Map<string, string[]>
  ) => {
    setSnapshots(newSnapshots);
    setCurrentSnapshotIndex(newIndex);
    setScrubToFn(() => scrubTo);
    if (newStateKeys) {
      setStateKeys(newStateKeys);
    }

    // v2.0: Update execution path based on snapshots
    // @see Requirements 10.3, 10.5: Execution path highlighting
    if (newSnapshots.length > 0) {
      executionPath.resetPath();
      executionPath.startExecution();
      newSnapshots.forEach(s => {
        executionPath.moveToNode(s.nodeId);
      });
    }
  }, [executionPath]);

  // Current and previous snapshots for StateInspector (v2.0)
  // @see Requirements 4.5, 5.4: Update inspector when timeline position changes
  const currentSnapshot = useMemo(() => {
    if (currentSnapshotIndex < 0 || currentSnapshotIndex >= snapshots.length) {
      return null;
    }
    return snapshots[currentSnapshotIndex];
  }, [snapshots, currentSnapshotIndex]);

  const previousSnapshot = useMemo(() => {
    const prevIndex = currentSnapshotIndex - 1;
    if (prevIndex < 0 || prevIndex >= snapshots.length) {
      return null;
    }
    return snapshots[prevIndex];
  }, [snapshots, currentSnapshotIndex]);

  // Handler for state inspector history selection
  const handleStateHistorySelect = useCallback((index: number) => {
    if (scrubToFn) {
      scrubToFn(index);
    }
  }, [scrubToFn]);

  return {
    // Execution state
    flowPhase,
    activeAgent,
    iteration,
    setIteration,
    thoughts,
    executionPath,

    // Timeline state
    timelineCollapsed,
    setTimelineCollapsed,
    snapshots,
    currentSnapshotIndex,
    scrubToFn,

    // State inspector
    showStateInspector,
    setShowStateInspector,
    currentSnapshot,
    previousSnapshot,

    // HITL
    interruptedNodeId,

    // Data flow overlay
    stateKeys,
    highlightedKey,

    // Handlers
    handleFlowPhase,
    handleActiveAgent,
    handleThought,
    handleKeyHover,
    handleToggleDataFlowOverlay,
    handleInterruptChange,
    handleSnapshotsChange,
    handleStateHistorySelect,
  };
}
