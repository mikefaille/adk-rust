# Graph Playbook

## Build sequence
1. schema
2. nodes
3. entry/direct edges
4. conditional edges
5. compile and invoke
6. checkpointer (optional — MemoryCheckpointer or SqliteCheckpointer)

## Debug checklist
- no missing entry
- no missing target nodes
- recursion limit set for cycles
- stream output inspected for route correctness
- durable resume: executor auto-resumes from last checkpoint on startup
- StreamEvent::Resumed emitted when resuming from checkpoint
