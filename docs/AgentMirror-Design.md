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
  llm_critic: true               # default on — dialectical + logical LLM reflection reports
  critic_model_group: high       # fallback group for insight LLM calls
```

When `llm_critic: true` (default), reflection reports are **LLM-only**: (1) infer **original goal** from the first 10 events; (2) batch remaining events (≤100k estimated tokens per batch) with iterative five-dimension critique, tracking **current goal** when the user shifts topic; (3) store **original goal + current goal + final reflection**. Rule baseline only when LLM is unavailable.

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
POST /api/insight/reset              # wipe insight_* + graphs; optional traffic replay
```

---

## 7. UI

Nav order: **AgentMirror** first.

- Agent list + run cards (checkbox merge)
- Trajectory modal: **Graph / Timeline / Events / Raw traffic**
- Graph tab: **directed graph** (layered DAG), **mind map** (radial from Goal), or list fallback; edges show causal labels
- Daily report date picker + viewer
- Reflection report modal: **五维审视** (score + narrative per dimension), summary, dialectical blocks when LLM enabled

---

## 8. Implementation status

| Capability | Status |
|------------|--------|
| Trace ingest (buffered + SSE) | ✅ |
| Run boundary (multi-turn) | ✅ |
| Rule parser / extractor / decision graph | ✅ |
| Five critics (rule-based) | ✅ |
| LLM goal + critic + dialectical/counterfactual | ✅ (default on; `llm_critic: false` to disable) |
| Safety critic ↔ ops rules | ✅ |
| Run merge / split | ✅ |
| Daily report SQL + Markdown files | ✅ (`data/insight/daily/`) |
| Raw traffic tab (audit → snapshot) | ✅ |
| Pattern mining / agent profile | ✅ V2 |
| DLP cross-highlight on runs | ✅ V2 |
| Daily report print / PDF export | ✅ V2 (browser print) |
| Multi-turn sessions (message delta, tool normalization, generic CN/EN patterns) | ✅ V2.1 |

**Multi-turn / OpenClaw-style sessions（V2.1 — 通用规则，无行业限定）**

认知事件抽取 **仅依赖对话结构与工具语义**，不绑定金融、投资、调研等垂直领域关键词。

- 每轮请求只处理 `messages[]` 增量（`messages_seen`），避免 Goal/Action/Observation 重复入库（OpenClaw 等客户端每轮重发全量 history）
- `exec`/shell 按 **参数语义** 归一化：含 `http`/`curl`/`search` 等 → `WebSearch`；否则 → `Exec`（与 goal 文案无关）
- **Decision**：统一中英文计划句正则（I'll / 我先 / 接下来 / 打算… + 通用动词：查询、实现、修复、运行…）
- **Result**：通用完成信号（done/完成/已修复）+ 结论文（结论、总结、综上、in conclusion…）+ 长度门槛；**不按行业词**（如投资建议）触发
- **Agent 类型** `explore`：由 tools/platform 推断（search/browser、OpenClaw/Hermes、exec-only），非 goal 关键词；旧数据 `research` 仍兼容显示
- **Critic** `TaskKind::Explore`：由 Action 序列推断（Edit/Write → Coding；有 Action → Explore）；Partial **不**提前将 Run 标为 Completed
- 推理图：Goal/SubGoal 作为根节点，不再从链尾错误挂接

**Reset / replay**

- `POST /api/insight/reset` — body `{"replay_from_traffic":false}` clears AgentMirror only
- `{"replay_from_traffic":true,"limit":5000}` also rebuilds from saved traffic snapshots (requires `save_traffic_bodies: true`)
- CLI: `./scripts/clear-insight.sh` (offline or live API); `--replay` when SMR is running

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
