# AgentMirror — Agent Cognitive Reconstruction

**Product module for LLM-SafeRoute.** Reconstruct what agents did, why, and how to improve — from LLM proxy traffic alone.

**Related docs**

| Document | Purpose |
|----------|---------|
| [`AgentMirror-Detailed-Plan.md`](AgentMirror-Detailed-Plan.md) | Full original implementation plan (canonical reference) |
| `Agent_Cognitive_Reconstruction_Design.docx`, `ACRP_Complete_Design.docx` | Source design documents |
| `Agent-Reflection-Prompt.txt` | Pipeline requirements checklist |

---

## 1. Positioning

| Before | After |
|--------|-------|
| LLM-SafeRoute = proxy + DLP + routing | **AgentMirror** = primary insight surface; routing/DLP remain infrastructure |

**Tagline:** *See what your agents actually did — and how to do it better.*

Optional request header for multi-agent clients: **`X-SMR-Agent-Id`**.

---

## 2. Constraints

1. **Multi-agent:** One traffic stream may contain multiple agents → Agent Separator.
2. **Daily reports:** Per-agent daily summary (tasks, issues, recommendations).
3. **Causal graph:** User-friendly modal (Goal → Decision → Action → Observation).
4. **Lightweight stack:** SQLite + JSON files + background queue — **no** ClickHouse, Neo4j, Kafka, Qdrant.
5. **Embedded:** Hooks into proxy, audit, Admin UI (traffic snapshots recommended).

---

## 3. Architecture

```
Proxy (existing)
  → inline TraceTurn submit (buffered or SSE tap)
  → InsightWorker (dedicated thread + Tokio runtime)
  → Conversation Parser
  → Agent Separator (agent_id + run_id)
  → Cognitive Event Extractor (rules; LLM optional in V1)
  → Reasoning Graph Builder → JSON on disk
  → Critic Engine (rules MVP)
  → Reflection Report + Daily Report
  → SQLite (insight_* tables) + Admin UI
```

With proxy-only logs we use **Cognitive Process Reconstruction**, not full Process Mining.

---

## 4. Multi-agent separation

| Priority | Signal | Status |
|----------|--------|--------|
| 1 | `X-SMR-Agent-Id` request header | ✅ Implemented |
| 2 | `sha256(system_prompt + tools[])` fingerprint | ✅ Implemented |
| 3 | First user message → Goal anchor | ✅ Goal inference |
| 4 | Run boundary: idle 30 min, explicit new-task markers, topic shift | ✅ Implemented |

```
Session (SMR session_id)
  └── Agent (system_hash + tools_hash)
        └── Run (one task: Goal → complete/fail)
```

**Run boundary rules** (see detailed plan §十二): continue active `running` run by default; start new run on idle timeout (30 min), completed/failed status, or explicit new-task signals (`new task:`, `/clear`, 新任务, etc.).

---

## 5. Data model

### CognitiveEvent kinds

`Goal | SubGoal | Decision | Action | Observation | Reflection | Result | StateTransition`

### Storage

- **SQLite** (`smr.db`, tables `insight_*`): agents, runs, events, reports, daily_reports, processed_audits
- **JSON files** (`data/insight/graphs/{run_id}.json`): reasoning graphs (linear chain MVP)

### ReflectionReport (output)

- goal, execution_summary, outcome, issues, risks, suggestions
- critics: alignment, necessity, completeness, efficiency, safety (0–100)
- dialectical notes → **V1** (LLM)

---

## 6. Configuration (`smr.yaml`)

```yaml
insight:
  enabled: true
  require_traffic_bodies: true   # auto-enables logging.save_traffic_bodies on load
  daily_report_hour: 8
  retention_days: 30           # purged on startup + daily scheduler
  llm_critic: false              # V1
  critic_model_group: medium
logging:
  save_traffic_bodies: true      # auto-set when insight.require_traffic_bodies
  traffic_retention_days: 7      # source snapshots may expire before insight retention
```

---

## 7. API

```
GET  /api/insight/status
GET  /api/insight/agents
GET  /api/insight/runs?agent_id=&limit=
GET  /api/insight/runs/{run_id}
GET  /api/insight/runs/{run_id}/graph
GET  /api/insight/runs/{run_id}/report
GET  /api/insight/daily/{date}?agent_id=
POST /api/insight/daily/generate
```

---

## 8. UI

Nav order (AgentMirror first):

**AgentMirror | Overview | Routing | DLP | …**

- Agent list + run cards
- **View trajectory** → modal with vertical causal graph + critic scores
- Daily report: date picker + **View daily report** + generate button

**Not yet in UI (V1):** modal tabs for timeline / raw transcript; Mermaid export.

Graph rendering: in-house vertical flow (linear chain); Decision Graph with branches → V1.

---

## 9. Crate layout

```
crates/smr-insight/
  src/
    lib.rs, models.rs, store.rs, parser.rs, separator.rs,
    extract.rs, graph.rs, critic.rs, report.rs, pipeline.rs, worker.rs

crates/smr-core/
  src/insight_admin.rs, insight_sse.rs   # SSE response tap for AgentMirror
```

Integration: `SharedApp.insight`, proxy hook after successful turn, `/api/insight/*` routes.

---

## 10. Implementation status

| Capability | Status | Notes |
|------------|--------|-------|
| Trace ingest (buffered JSON) | ✅ | Proxy inline submit |
| Trace ingest (SSE streams) | ✅ | `insight_sse::wrap_sse_for_insight` |
| Agent separation (header + fingerprint) | ✅ | |
| Run boundary (multi-turn) | ✅ | Fixed: no longer splits every turn |
| Rule parser / extractor / linear graph | ✅ | |
| Five critics (rule-based) | ✅ | Safety uses ops/path rules via `SafetyScanner` |
| Admin UI: agents, runs, graph modal | ✅ | Modal tabs: graph / timeline / events |
| Daily report backend | ✅ | SQL aggregation |
| Daily report viewer UI | ✅ | Date picker + panel |
| `retention_days` purge | ✅ | Startup + daily scheduler |
| Auto-enable `save_traffic_bodies` | ✅ | On config load |
| `X-SMR-Agent-Id` documented | ✅ | This doc + proxy support |
| Decision graph branching | ✅ | Actions branch from Decision nodes |
| Safety critic ↔ ops rules | ✅ | `OpsSafetyScanner` + `insight_policy_match` |
| Modal timeline / raw transcript tabs | ✅ | Timeline + events table |
| LLM Goal Discovery + critics | 🔲 V1 | |
| Dialectical / counterfactual | 🔲 V1 | |
| Manual run merge/split | 🔲 V1 | |
| Pattern mining / agent profile | 🔲 V2 | |
| Markdown daily file export | 🔲 V2 | |

Legend: ✅ shipped · 🔲 planned

---

## 11. Phases (roadmap)

### MVP — delivered

Core ingest → separate → extract → graph → critics → Admin UI → daily reports.

### V1

- LLM Goal Discovery + five critics + counterfactual
- Manual run merge/split
- Raw traffic body viewer in graph modal (link audit → traffic snapshot)

### V2

- Success/failure pattern mining (SQLite sequences)
- Agent capability profile from system + tools
- DLP/safety cross-highlight
- Daily report Markdown / PDF export

---

## 12. Performance

- Proxy path: non-blocking `try_send` only; queue full → drop + warn
- Worker: dedicated thread + Tokio runtime (GUI-safe)
- Rule pipeline: target < 200 ms per turn (background)
- LLM critic: opt-in, 1–2 calls per run (V1)

---

## 13. Risks & known limitations

| Risk | Mitigation |
|------|------------|
| Traffic off | Auto-enable when `insight.require_traffic_bodies`; UI warning if still off |
| Traffic retention (7d) < insight retention (30d) | Increase `traffic_retention_days` for long analysis windows |
| Wrong goal inference | Show confidence; user edit (V1) |
| Multi-agent mis-split | `X-SMR-Agent-Id`; manual fix (V1) |
| Linear graph oversimplifies decisions | Decision Graph in V1 |
| SSE body truncated | Same limit as `traffic_max_body_bytes` |
