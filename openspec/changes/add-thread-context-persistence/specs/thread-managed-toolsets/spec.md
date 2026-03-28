## MODIFIED Requirements

### Requirement: System SHALL persist thread toolset state and tool call records
The system SHALL persist loaded toolset state and structured tool call records inside the thread persistence snapshot so runtime reconstruction and audit remain possible across process restarts. Toolset recovery SHALL use the persisted thread snapshot as the source of truth, and restarting the service SHALL NOT collapse toolset isolation between different internal threads.

#### Scenario: Thread runtime can be reconstructed from persisted state
- **WHEN** a thread with loaded toolsets is reloaded from the persisted thread snapshot after a process restart
- **THEN** the runtime can restore that thread's loaded toolset set before serving the next model request
- **THEN** the persisted thread record contains structured evidence of the tool load, unload, and execution history
- **THEN** other internal threads still keep their own independent loaded toolset state
