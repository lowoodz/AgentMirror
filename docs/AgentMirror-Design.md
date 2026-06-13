# AgentMirror — Agent Cognitive Reconstruction

**Product module for LLM-SafeRoute.** Reconstruct what agents did, why, and how to improve — from LLM proxy traffic alone.

References: `Agent_Cognitive_Reconstruction_Design.docx`, `ACRP_Complete_Design.docx`, `Agent-Reflection-Prompt.txt`.

---

## 1. Positioning

| Before | After |
|--------|-------|
| LLM-SafeRoute = proxy + DLP + routing | **AgentMirror** = primary insight surface; routing/DLP remain infrastructure |

**Tagline:** *See what your agents actually did — and how to do it better.*

---

## 2. Constraints

1. **Multi-agent:** One traffic stream may contain multiple agents → Agent Separator.
2. **Daily reports:** Per-agent daily summary (tasks, issues, recommendations).
3. **Causal graph:** User-friendly modal (Goal → Decision → Action → Observation).
4. **Lightweight stack:** SQLite + JSON files + Tokio queue — **no** ClickHouse, Neo4j, Kafka, Qdrant.
5. **Embedded:** Hooks into existing `TrafficLog`, `AuditStore`, Admin UI.

---

## 3. Architecture

```
Proxy (existing)
  → TrafficLog (request_out + response_out, audit_id)
  → Trace Ingestor (InsightWorker)
  → Conversation Parser
  → Agent Separator (agent_id + run_id)
  → Cognitive Event Extractor (rules; LLM optional in V1)
  → Reasoning Graph Builder → JSON on disk
  → Critic Engine (rules MVP)
  → Reflection Report + Daily Report
  → SQLite (insight_* tables) + Admin UI
```

Pipeline mapping (from `Agent-Reflection-Prompt.txt`):

```
Agent Log → Trace Collector → Trajectory Parser → Action Extraction
→ Workflow Discovery → Reasoning Graph → Goal/Plan/Control Flow/Decision Mining
→ Behavior Graph → Rule Engine + LLM Critic + Safety Auditor → Reflection Report
```

With proxy-only logs we use **Cognitive Process Reconstruction**, not full Process Mining.

---

## 4. Multi-agent separation

| Priority | Signal |
|----------|--------|
| 1 | `X-SMR-Agent-Id` request header |
| 2 | `sha256(system_prompt + tools[])` fingerprint |
| 3 | First user message task anchor |
| 4 | System/tools change → new run boundary |

```
Session (SMR session_id)
  └── Agent (system_hash + tools_hash)
        └── Run (one task: Goal → complete/fail)
```

---

## 5. Data model

### CognitiveEvent kinds

`Goal | SubGoal | Decision | Action | Observation | Reflection | Result | StateTransition`

### Storage

- **SQLite** (`smr.db`, tables `insight_*`): agents, runs, events, reports, daily_reports
- **JSON files** (`data/insight/graphs/{run_id}.json`): reasoning graphs

### ReflectionReport (output)

- goal, execution_summary, outcome, issues, risks, suggestions
- critics: alignment, necessity, completeness, efficiency, safety (0–100)
- dialectical notes (V1 with LLM)

---

## 6. Configuration (`smr.yaml`)

```yaml
insight:
  enabled: true
  require_traffic_bodies: true
  daily_report_hour: 8
  retention_days: 30
  llm_critic: false          # V1
  critic_model_group: medium
```

When `insight.enabled` and `require_traffic_bodies`, traffic snapshots should be on.

---

## 7. API

```
GET  /api/insight/status
GET  /api/insight/agents
GET  /api/insight/runs?agent_id=&limit=
GET  /api/insight/runs/{run_id}
GET  /api/insight/runs/{run_id}/graph
GET  /api/insight/runs/{run_id}/report
GET  /api/insight/daily/{date}
POST /api/insight/daily/generate
```

---

## 8. UI

Nav order (AgentMirror first):

**AgentMirror | Overview | Routing | DLP | …**

- Agent list + run cards
- **View trajectory** → modal with vertical causal graph + critic scores
- Daily report viewer

Graph rendering: in-house vertical flow (no Neo4j); optional Mermaid export later.

---

## 9. Crate layout

```
crates/smr-insight/
  src/
    lib.rs
    models.rs
    store.rs      # SQLite insight_* 
    parser.rs     # OpenAI/Anthropic messages
    separator.rs  # agent_id, run_id
    extract.rs    # rule-based events
    graph.rs      # ReasoningGraph JSON
    critic.rs     # completeness, efficiency, safety rules
    report.rs     # reflection + daily
    pipeline.rs   # orchestration
    worker.rs     # async queue
```

Integration in `smr-core`: `SharedApp.insight`, proxy hook after successful turn, `admin` routes.

---

## 10. Phases

### MVP (current)

- Trace ingestor + SQLite schema
- Rule-based parser / extractor / graph
- Basic critics (completeness, efficiency, safety)
- Admin UI: agents, runs, graph modal
- Daily report (SQL aggregation)

### V1

- LLM Goal Discovery + five critics + counterfactual
- `X-SMR-Agent-Id` documented
- Manual run merge/split

### V2

- Success/failure pattern mining (SQLite sequences)
- Agent capability profile from system + tools
- DLP/safety cross-highlight

---

## 11. Performance

- Proxy path: non-blocking `enqueue` only
- Rule pipeline: target &lt; 200 ms per turn (background)
- LLM critic: opt-in, 1–2 calls per run (V1)

---

## 12. Risks

| Risk | Mitigation |
|------|------------|
| Traffic off | UI warning; auto-enable when insight on |
| Wrong goal inference | Show confidence; user edit (V1) |
| Multi-agent mis-split | `X-SMR-Agent-Id`; manual fix (V1) |
