# Observational Memory Technical Documentation for Quick‑Query

---

## Overview
Observational Memory (OM) is a three‑tier, **text‑only memory system** that replaces the raw message history in an agent with a compact *observation log*. By continuously summarising new conversations into concise observations and periodically compressing those observations, OM provides a stable prompt prefix that can be cached across turns. This enables **prompt‑cacheability**, drastically reduces token usage, and offers instant, zero‑cost look‑ups through the Quick‑Query layer.

The design is purpose‑built for Quick‑Query: after each reflection the observation log becomes immutable, allowing rapid filter‑only queries (`count`, `filter`, `exists`) without any vector database or embedding lookup. The result is a fast, deterministic, and auditable long‑term memory subsystem.

---

## Design Goals
- **Token Compression** – Turn raw chat logs (often > 200 k tokens) into an observation log typically < 30 k tokens.
- **Prompt Cacheability** – Keep the prefix unchanged after each reflection, allowing LLM services to reuse cached prompts across turns.
- **Fast In‑Memory Queries** – Enable `quickQuery` operations that run in O(N) over a bounded text buffer (≈ 10–30 k tokens).
- **Auditability & Traceability** – Store immutable observations with timestamps and priority tags, providing an audit trail.
- **Deterministic Latency & Cost** – Bounded token budgets guarantee predictable query latency and lower inference costs.
- **Security‑Ready** – Centralised storage of observations simplifies redaction, retention policies, and access control.

---

## Architecture Diagram (textual description)
The OM pipeline consists of **three tiers** that flow from raw messages to a stable observation log:
```
Message History → Observer → Observation Log → Reflector → Reflected Log → Quick‑Query Layer
```
- **Message History** – Raw user, assistant, and tool output accumulated each turn until the *message‑token threshold* (≈ 2–3 k tokens) is reached.
- **Observer Agent** – Triggered when the message threshold is exceeded; it summarises the newest chunk into one or more **observations**, preserving emojis for priority, timestamps, actor tags, and optional key/value labels.
- **Observation Log** – An append‑only text block of observation entries. It grows continuously as observers run.
- **Reflector Agent** – Triggered when the observation log exceeds an *observation‑token threshold* (≈ 10–30 k tokens). It merges related observations, drops low‑priority items, and rewrites the log into a tighter **reflected log**.
- **Quick‑Query Layer** – Exposes `memory.quickQuery` APIs that perform fast regex/keyword scans over the reflected observation log without any embeddings.
```
The three‑tier pipeline guarantees that after each reflection the prompt prefix is static, enabling seamless prompt caching.
```
---

## Key Components
| Component | Role |
|-----------|------|
| **Observer** | Summarises newest message chunk into observations using a small LLM (e.g., `gemini‑2.5‑flash`). |
| **Reflector** | Periodically compresses the observation log, merging and pruning entries while preserving priority information. |
| **Observation Log Entry** | Text bullet with:
- Emoji priority (`🔴` high, `🟡` medium, `🟢` low)
- Timestamp (observation date)
- Actor tag (`USER`, `TOOL CALL`, `ASSISTANT`)
- Content description
- Optional tags (e.g., `label:`, custom key/value pairs) |
| **Quick‑Query Layer** | Provides `count`, `filter`, and `exists` methods that operate on the reflected log via fast regex scanning. |
| **Memory Configuration** | Enables OM and Quick‑Query via the `Memory` constructor (`observationalMemory: true`, `quickQueryEnabled: true`). |

---

## Data Flow (Turn‑by‑Turn Walkthrough)
1. **Message Accumulation** – The agent collects user messages, assistant replies, and tool outputs in `Message History`.
2. **Observer Trigger** – Once the message token budget (~2–3 k tokens) is exceeded, the Observer runs on the newest chunk.
3. **Observation Creation** – The Observer emits one or more observations (emoji priority, timestamp, tags) which are appended to the Observation Log.
4. **Reflection Check** – If the Observation Log size exceeds its token budget (~10–30 k tokens), the Reflector is invoked.
5. **Reflector Processing** – It merges related entries, discards low‑priority (`🟢`) items, and rewrites a tighter reflected log while preserving three‑date anchoring (observation, referenced, relative dates).
6. **Prompt Construction** – The final LLM prompt for the next turn is assembled as:
   ```
   [system_prompt] + [reflected_observation_log] + [current_message_history]
   ```
   After reflection the `[reflected_observation_log]` does not change, ensuring a cache‑hit on subsequent turns.
7. **Quick‑Query Usage** – At any point an agent can query the observation log via `memory.quickQuery.filter({...})`, `count(...)`, or `exists(...)`. These queries parse the in‑memory text and return results instantly (typically < 10 ms).

---

## Algorithms
### Observer Algorithm
- **Input**: Latest message chunk (≤ message‑token threshold).
- **LLM Prompt**: Small, cheap LLM (`gemini‑2.5‑flash` by default) instructed to generate observations.
- **Output**: One or more observation entries containing emojis for priority, timestamps, actor tags, and optional `label:` tags.
- **Append**: Entries are appended verbatim to the Observation Log.

### Reflector Algorithm
- **Trigger**: Observation log token count > observation‑token threshold.
- **LLM Prompt**: Larger LLM (e.g., GPT‑4o) asked to:
  - Merge related observations (same topic/date).
  - Drop superseded or low‑priority (`🟢`) entries.
  - Preserve the three‑date model (observation, referenced, relative dates).
- **Result**: A compact *reflected log* that replaces the previous Observation Log content.
- **Compression Ratio**: Typically 3–6× for pure chat; up to 5–40× when tool output dominates.

### Token Budgets & Compression
| Budget Type | Typical Size |
|-------------|--------------|
| Message‑token threshold | ~2 k – 3 k tokens |
| Observation‑token threshold | ~10 k – 30 k tokens |

These thresholds keep the observation log bounded, guaranteeing deterministic `quickQuery` latency.

### Quick‑Query Processing
- The reflected log is stored as plain text in the `Memory` object's `observationLog` field.
- Queries are implemented as fast **regex/keyword scans** over this text (O(N) where N ≤ 30 k tokens).
- Supported primitives:
  - `count(filter)` – Number of matching observations.
  - `filter(filter)` – Returns the raw observation strings that satisfy the filter.
  - `exists(filter)` – Boolean existence check.
- No embeddings, vector DB, or external services are involved, resulting in sub‑10 ms response times.

---

## APIs / Integration
### Enabling Observational Memory
```ts
import { Memory } from "quick-query";

const memory = new Memory({
  observationalMemory: true,   // turn on Observer + Reflector pipeline
  quickQueryEnabled: true      // expose fast query API
});
```
### Quick‑Query Methods
| Method | Signature (example) | Description |
|--------|---------------------|-------------|
| `count(filter)` | `await memory.quickQuery.count({ label: 'error', after: new Date(Date.now() - 86_400_000) })` | Returns the number of matching observations.
| `filter(filter)` | `await memory.quickQuery.filter({ emoji: '🔴' })` | Retrieves observation strings that match the filter.
| `exists(filter)` | `await memory.quickQuery.exists({ label: 'api-rate-exceeded' })` | Boolean check – useful for gating logic.

### Simple Usage Example
```ts
// Count recent high‑priority error events in the last hour
const errorCount = await memory.quickQuery.count({
  emoji: "🔴",
  after: new Date(Date.now() - 60 * 60 * 1000) // 1 h ago
});
if (errorCount > 5) {
  // Too many errors – switch to a fallback tool or throttle calls.
  await someFallbackStrategy();
}
```
The query runs in memory instantly and does **not** add tokens to the LLM prompt.

---

## Usage Examples
### Gating Tool Calls Based on Recent Errors
```ts
async function maybeCallExternalApi(params) {
  const recentRateLimits = await memory.quickQuery.exists({
    label: "api-rate-exceeded",
    after: new Date(Date.now() - 5 * 60 * 1000) // last 5 min
  });
  if (recentRateLimits) {
    console.log("Skipping external API – recent rate limit violations detected.");
    return { fallback: true };
  }
  // Safe to proceed with the real call
  const result = await externalApi(params);
  return result;
}
```
---
### Summarising All Critical Errors Over a Day
```ts
async function summarizeDailyErrors() {
  const errorObs = await memory.quickQuery.filter({
    emoji: "🔴",
    after: new Date(Date.now() - 24 * 60 * 60 * 1000) // last 24 h
  });
  const summary = errorObs.join("\n");
  console.log("Critical errors in the past day:\n", summary);
}
```
These snippets illustrate how a Quick‑Query‑enabled agent can make rapid decisions without invoking an LLM for each check.

---

## Performance & Security Considerations
### Performance Summary
| Metric | Observational Memory (OM) |
|--------|---------------------------|
| **Compression Ratio** | ~3–6× for chat logs; up to 5–40× when tool output dominates. |
| **Typical Reflected Log Size** | ≈ 30 k tokens (vs. ~200 k raw messages). |
| **Observer/Reflector Latency** | Runs asynchronously; each pass < 1 s for typical workloads. |
| **Quick‑Query Query Latency** | < 10 ms (regex over ≤ 30 k token text). |
| **Token Savings** | 4–10× fewer tokens per turn versus RAG pipelines. |
| **Cache Hit Rate** | Near‑100% after first reflection; each subsequent turn reuses the same cached prefix. |

### Security & Privacy Mitigations
- **Redaction** – The Observer can be configured to mask PII (e.g., email addresses, credit card numbers) before logging observations.
- **Encryption at Rest** – Store the observation log on encrypted volumes; Mastra’s `Memory` supports encrypted back‑ends.
- **Retention Policies** – Configure stricter observation‑token thresholds or explicit TTLs to prune old observations after they have been reflected.
- **Access Control** – Enforce role‑based permissions on the `memory.quickQuery` object so only privileged agents can read the full log; others receive filtered views.
- **Auditability** – Because observations are immutable until reflection, they provide a tamper‑evident audit trail. Hashes of each reflected version can be signed for compliance.

---

## References
- Mastra AI Blog: *Observational Memory: 95 % on LongMemEval* – https://mastra.ai/research/observational-memory
- Quick‑Query Documentation (internal) – `Memory` API reference and configuration guide.
- Related research on prompt caching and long‑term LLM memory architectures.

---

*Document generated automatically from the Observational Memory technical summary.*