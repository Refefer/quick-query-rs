# Observational Memory: Technical Implementation Reference

Source: [Mastra AI — Observational Memory: 95% on LongMemEval](https://mastra.ai/research/observational-memory)

---

## 1. What Observational Memory Is

Observational Memory (OM) is a three-tier context management architecture that replaces
raw conversation history with a continuously-maintained, append-only observation log.
An **Observer** agent watches the conversation and distills it into dated, prioritized
observations. A **Reflector** agent periodically restructures and compresses those
observations when they grow past a token budget. The result is a stable, bounded
context prefix that is prompt-cacheable across turns.

OM scored **94.87%** on LongMemEval (GPT-5-mini) — the highest recorded score on that
benchmark by any system. With GPT-4o it scored 84.23%, beating both the full-context
baseline (+24 points) and the oracle retrieval baseline (+2 points).

### OM vs. Traditional Compaction

Traditional compaction bulk-summarizes messages when context overflows, producing
documentation-like prose that loses specific events and decisions. OM instead maintains
an **event-based decision log** — a structured list of dated, prioritized observations
about what specifically happened, what was decided, and what changed. It is append-only
until a reflection pass rewrites the log, preserving all individual events rather than
collapsing them into narrative summaries.

---

## 2. Three-Tier Architecture

```
 ┌──────────────────┐
 │  Message History  │  Raw user/assistant/tool messages. Grows each turn.
 └────────┬─────────┘
          │  message-token threshold exceeded
          ▼
 ┌──────────────────┐
 │     Observer      │  Background agent: converts messages → observations.
 └────────┬─────────┘
          │  append
          ▼
 ┌──────────────────┐
 │ Observation Log   │  Append-only list of dated, prioritized entries.
 └────────┬─────────┘
          │  observation-token threshold exceeded
          ▼
 ┌──────────────────┐
 │    Reflector      │  Restructures observations: merge, prune, condense.
 └────────┬─────────┘
          │  replace
          ▼
 ┌──────────────────┐
 │  Reflected Log    │  Stable prefix for the LLM prompt. Cacheable.
 └──────────────────┘
```

### Tier 1 — Message History

Raw conversation messages (user, assistant, tool calls, tool results). This is the
most recent, unprocessed context. It accumulates until it hits the **message-token
threshold**, at which point the Observer processes the oldest unobserved chunk.

### Tier 2 — Observations

The Observer converts raw messages into dense, structured observations. Each
observation is a concise note capturing a specific event: a user statement, an agent
action, a tool call result, a preference expressed in passing. Observations replace
the raw messages they were derived from — the originals are dropped from the prompt.

**Compression ratios:**
- Text-only conversations: **3–6x**
- Tool-call-heavy workloads: **5–40x** (tool output is highly compressible)

### Tier 3 — Reflections

When the observation log exceeds its token budget, the Reflector restructures it:
combining related items, identifying patterns, and removing observations that have
been superseded. This is not summarization — it produces a tighter version of the
same structured observation format.

---

## 3. The Observer

### Behavior

The Observer is a background agent that monitors the conversation. It runs whenever
unobserved messages exceed the message-token threshold. It does **not** run on every
turn — only when enough new material has accumulated.

### Output Format

Observations use **formatted text**, not structured objects (no JSON, no schemas).
The format is:

- **Two-level bulleted lists**: top-level bullets for tasks/events, sub-bullets for
  supporting details
- **Emoji-based priority**: `🔴` high, `🟡` medium, `🟢` low
- **Titles and timestamps**: organized by date
- **Temporal anchoring** (three dates per observation):
  - **Observation date** — when the observation was created
  - **Referenced date** — any date mentioned in the content ("meeting on March 5")
  - **Relative date** — computed offset ("2 days from today")

### Why Text, Not Structured Data

Formatted text is directly consumable by the LLM without parsing or serialization.
It compresses well, can be prompt-cached as a string prefix, and avoids the overhead
of maintaining schemas or deserializing structured objects at inference time.

### Append-Only Semantics

The observation log is **append-only** between reflection passes. This is critical
for two reasons:

1. **Context stability** — the prompt prefix does not change between reflections,
   enabling high cache hit rates.
2. **Prompt caching** — many providers reduce token costs 4–10x for cached prefixes.
   An append-only log maximizes the cacheable portion.

---

## 4. The Reflector

### Trigger

The Reflector runs when the total observation log size exceeds the
**observation-token threshold**.

### Algorithm

1. Read the full observation log.
2. **Merge** related observations (same topic, same date range, same entity).
3. **Prune** low-priority (`🟢`) entries that have been superseded by later events.
4. **Preserve** the three-date model on all surviving entries.
5. **Rewrite** the log in the same structured observation format (not prose).
6. **Replace** the old observation log with the reflected version.

### What the Reflector Does NOT Do

- It does not produce narrative summaries ("The user discussed X and then Y...").
- It does not discard information arbitrarily — only superseded or low-priority entries.
- It does not change the format. Output is still dated, prioritized, bulleted observations.

---

## 5. Temporal Anchoring

The three-date model is one of OM's key differentiators. Each observation carries:

| Date Type | Example | Purpose |
|-----------|---------|---------|
| Observation date | 2026-02-17 | When the observation was recorded |
| Referenced date | 2026-03-05 | A date mentioned in the content |
| Relative date | "16 days from today" | Computed offset for temporal reasoning |

This proved critical for temporal reasoning tasks on LongMemEval, where OM scored
**95.5%** (GPT-5-mini). Without relative dates, the model must compute offsets from
absolute dates — a task LLMs are notoriously bad at.

---

## 6. Token Budgets

Two configurable thresholds control the pipeline:

| Threshold | Controls | Effect |
|-----------|----------|--------|
| Message-token threshold | Unobserved message history | Triggers the Observer when exceeded |
| Observation-token threshold | Total observation log size | Triggers the Reflector when exceeded |

The blog does not specify exact default values, but the average context window size
across LongMemEval runs was **~30k tokens** total (observations + recent messages).

### Prompt Assembly

Each turn, the LLM prompt is assembled as:

```
[system prompt] + [reflected observation log] + [recent message history]
```

After a reflection pass, the `[reflected observation log]` segment is frozen until
the next reflection — enabling the provider to cache it.

---

## 7. Benchmark Results

### LongMemEval Scores

| Model | Overall | Knowledge Update | Single-Session Pref | Temporal Reasoning | Single-Session Asst | Single-Session User | Multi-Session |
|-------|---------|-----------------|---------------------|-------------------|--------------------|--------------------|---------------|
| GPT-4o | 84.23% | — | 73.3% | 85.7% | 82.1% | — | — |
| Gemini-3-Pro | 93.27% | — | — | — | — | — | — |
| GPT-5-mini | **94.87%** | 96.2% | 100% | 95.5% | 94.6% | 95.7% | 87.2% |

### Key Comparisons

- **vs. Full-context baseline (GPT-4o)**: OM beats it by 24 points despite using
  compressed observations instead of raw transcripts.
- **vs. Oracle retrieval (GPT-4o)**: OM beats the oracle by 2 points. The oracle
  is given only the 1–3 conversations containing the answer; OM ingests all ~50
  conversations per question and still extracts better signal.
- **vs. Hindsight** (prior SOTA): OM has a 3.5-point advantage despite Hindsight
  using four parallel retrieval strategies with neural reranking. OM achieves this
  with a single pass and no retrieval steps.

### Model Scaling

OM benefits more from better models than competing approaches:

- Supermemory: 81.6% → 85.2% (GPT-4o → Gemini-3-Pro) = +3.6 points
- OM: 84.23% → 93.27% (GPT-4o → Gemini-3-Pro) = **+9.0 points**

The hypothesis is that stronger models extract more value from dense, structured
observations than weaker ones.

---

## 8. Why OM Outperforms RAG

Traditional RAG-based memory retrieves dynamically selected context every turn,
which means:

1. The prompt changes every turn → no prompt caching.
2. Retrieval quality is a bottleneck → missed relevant context.
3. Multiple retrieval strategies add latency and complexity.

OM avoids all three problems:

1. **Stable prefix** — the observation log changes only at reflection boundaries.
2. **No retrieval** — all observations are always in context; nothing can be missed.
3. **Single pass** — no embedding, no reranking, no vector DB.

The trade-off is that OM relies on the LLM to attend over a larger (but bounded)
observation log, while RAG tries to minimize context size through selective retrieval.
With modern long-context models, OM's approach wins.

---

## 9. Limitations

### Multi-Session Reasoning

Multi-session questions (synthesizing information across separate conversations) are
the hardest category. OM scores 87.2% here, tying Hindsight's best. This appears to
be a ceiling for current systems — the challenge is that related information is
scattered across conversations with no explicit links.

### Statistical Noise in Small Categories

Single-session-preference has only 30 questions, so a single-question swing moves
scores by 3.3 percentage points. GPT-4o scored 73.3% while GPT-5-mini scored 100%
on this category — a 26.7-point gap likely amplified by small sample size.

### GPT-4o Performance Gaps

GPT-4o shows clear weaknesses versus GPT-5-mini on dense observation logs:
- Temporal reasoning: -9.8 points
- Single-session preference: -26.7 points
- Single-session assistant: -12.5 points

This suggests older/weaker models struggle with the information density that OM
produces. The system works best with capable models.

---

## 10. Design Principles for Implementation

These are the architectural properties that make OM work, distilled for adaptation:

1. **Observe, don't summarize.** Produce discrete, dated, prioritized event records —
   not prose summaries. Each observation should capture one specific thing that happened.

2. **Append-only between reflections.** Never mutate the observation log outside of a
   full reflection pass. This is what enables prompt caching.

3. **Three-date temporal anchoring.** Every observation carries creation time, referenced
   time, and a relative offset. This is not optional — it is responsible for the 95.5%
   temporal reasoning score.

4. **Formatted text, not structured data.** Observations are bulleted text with emoji
   priority markers. The LLM consumes them directly without parsing.

5. **Two-threshold pipeline.** Separate budgets for "when to observe" and "when to
   reflect" decouple the compression stages and allow independent tuning.

6. **Reflection preserves structure.** The reflector outputs the same observation
   format, just tighter. It merges and prunes but does not change the representation.

7. **No retrieval step.** All observations are always in context. The model attends
   over them directly. This eliminates retrieval as a failure mode.

---

## 11. Implementation Considerations for a Rust Agent System

Adapting OM to a Rust-based agent framework (like quick-query) involves:

### Storage

The observation log is plain text. It can be stored as a `String` field on the session
object. No database, no embeddings, no vector store. Persistence across sessions
requires serializing this string to disk.

### Observer Integration

The Observer is itself an LLM call. It can be implemented as:
- A dedicated agent with a fixed system prompt instructing observation extraction
- Triggered by a byte/token count check on unobserved messages
- Input: the unobserved message slice
- Output: formatted observation text, appended to the log

### Reflector Integration

The Reflector is also an LLM call. It can be:
- A dedicated agent with a system prompt for restructuring observations
- Triggered by a byte/token count check on the observation log
- Input: the full observation log
- Output: a replacement observation log (same format, smaller)

### Prompt Assembly

The prompt for each turn becomes:
```
system_prompt + observation_log + recent_messages
```

Where `observation_log` is the stable, cacheable prefix. The `recent_messages` are
the raw messages that haven't been observed yet.

### Interaction with Existing Compaction

OM can coexist with or replace the existing tiered compaction system. The key
difference: compaction summarizes and discards; OM observes and restructures.
They serve the same goal (bounded context) but OM preserves more retrievable detail.

### Token Counting

The two thresholds need byte or token counting. Since the observation log is plain
text, byte counting (as already implemented in `Message::byte_count()`) is a
reasonable proxy. A ratio of ~4 bytes per token is typical for English text.

---

## References

- [Mastra AI — Observational Memory: 95% on LongMemEval](https://mastra.ai/research/observational-memory)
- [LongMemEval Benchmark](https://github.com/xiaowu0162/LongMemEval)
- [Mastra Observational Memory Source Code](https://github.com/mastra-ai/mastra) (open source)
