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
  → Cognitive Event Extractor (rules; LLM optional)
  → Reasoning Graph Builder → JSON on disk
  → Critic Engine (rules + optional LLM)
  → Reflection Report + Daily Report
  → SQLite (insight_* tables) + Admin UI
```

---

## 4. Multi-agent separation

| Priority | Signal | Status |
|----------|--------|--------|
| 1 | `X-SMR-Agent-Id` request header | ✅ |
| 2 | `sha256(system_prompt + tools[])` fingerprint | ✅ |
| 3 | First user message → Goal anchor | ✅ |
| 4 | Run boundary: idle 30 min, explicit new-task markers | ✅ |

---

## 5. Configuration (`smr.yaml`)

```yaml
insight:
  enabled: true
  require_traffic_bodies: true   # auto-enables logging.save_traffic_bodies
  daily_report_hour: 8
  retention_days: 30
  llm_critic: false              # set true to enable LLM goal + critic enrichment
  critic_model_group: medium     # fallback group for insight LLM calls
```

When `llm_critic: true`, AgentMirror calls the configured SafeRoute model group (1–2 calls per completed run; trajectory truncated to ~6k chars).

---

## 6. API

```
GET  /api/insight/status
GET  /api/insight/agents
GET  /api/insight/runs?agent_id=&limit=
GET  /api/insight/runs/{run_id}
PATCH /api/insight/runs/{run_id}          # edit goal
POST /api/insight/runs/merge              # merge runs
POST /api/insight/runs/{run_id}/split     # split after seq
GET  /api/insight/runs/{run_id}/graph
GET  /api/insight/runs/{run_id}/report
GET  /api/insight/audit/{audit_id}/traffic
GET  /api/insight/daily/{date}?agent_id=
POST /api/insight/daily/generate
```

---

## 7. UI

Nav order: **AgentMirror** first.

- Agent list + run cards (checkbox merge, edit goal)
- Trajectory modal: **Graph / Timeline / Events / Raw traffic**
- Daily report date picker + viewer
- Reflection report with dialectical + counterfactual (when LLM enabled)

---

## 8. Implementation status

| Capability | Status |
|------------|--------|
| Trace ingest (buffered + SSE) | ✅ |
| Run boundary (multi-turn) | ✅ |
| Rule parser / extractor / decision graph | ✅ |
| Five critics (rule-based) | ✅ |
| LLM goal + critic + dialectical/counterfactual | ✅ (`llm_critic: true`) |
| Safety critic ↔ ops rules | ✅ |
| Run merge / split + goal edit | ✅ |
| Daily report SQL + Markdown files | ✅ (`data/insight/daily/`) |
| Raw traffic tab (audit → snapshot) | ✅ |
| Pattern mining / agent profile | ✅ V2 |
| DLP cross-highlight on runs | ✅ V2 |
| Daily report print / PDF export | ✅ V2 (browser print) |

---

## 9. Phases

### V1 — delivered

Core MVP + ops safety + graph tabs + merge/split + LLM enrichment + daily Markdown.

### V2 — delivered

- Success/failure action pattern mining (`GET /api/insight/agents/{id}/patterns`)
- Agent capability profile from tools + run stats (`GET /api/insight/agents/{id}/profile`)
- DLP / ops safety cross-highlight on run cards (audit join)
- Daily report HTML print view (`GET /api/insight/daily/{date}/print`)

### V3 — planned

Backlog (also in local `TODO.txt`, gitignored). Not in the original Detailed-Plan; candidates from ACRP / Reflection Prompt.

**P0 — V2 doc gaps**

- [ ] Daily report email subscription
- [ ] Server-side PDF export (V2: browser Print/HTML only)

**P1 — Cognitive mining**

- [ ] Workflow discovery (cross-run)
- [ ] Control-flow mining (phases / branches)
- [ ] Plan inference (sub-goal tree)
- [ ] Decision mining + counterfactual DAG
- [ ] Behavior graph (vs Reasoning Graph)
- [ ] Full process mining (phase templates, bottlenecks, deviation — beyond V2 sequence similarity)

**P2 — Org & alerts**

- [ ] Cross-agent comparison / org daily report
- [ ] Alerts: high-risk runs, repeated failure patterns (webhook / email)
- [ ] Agent Digital Twin: baseline trajectories, success-rate drift

**P3 — Quality & scale**

- [ ] Completeness phase templates (Gathering → Implementing → Verifying)
- [ ] Long-session tiered summarization / sampling (billion-token sessions)

---

## 10. Performance & token budget

- Proxy path: non-blocking `try_send` only
- Rule pipeline: < 200 ms per turn (background)
- LLM: opt-in; compact trajectory (~6k chars); runs on completed/failed status + turn-1 goal refine
- Long sessions: insight retention 30d vs traffic 7d — increase `traffic_retention_days` if needed
