# Prompt Generalization Update Plan

**Document Status:** Draft  
**Created:** 2026-03-09  
**Target File:** `docs/prompt-generalization-update.md`

---

## Executive Summary

This document outlines a comprehensive update to the quick-query agent framework prompts to generalize agents for broader task applicability while preserving their effectiveness on coding tasks. The current prompts are heavily optimized for software development scenarios, which limits reusability for other domains like data analysis, content creation workflows, or system administration.

### Key Objectives

1. **Decouple domain-specific language** from core agent instructions
2. **Preserve coding task effectiveness** through targeted examples and strategies
3. **Enable cross-domain adaptability** without requiring prompt modifications
4. **Maintain clear role boundaries** while allowing flexible application

### Expected Impact

- **Reduced maintenance burden**: Single prompt set works across multiple domains
- **Improved user experience**: Agents naturally adapt to varied use cases
- **Preserved quality**: Coding tasks maintain the same level of effectiveness through domain-specific sections

---

## Current State Analysis

### Agent Inventory & Assessment

| Agent | Primary Domain Focus | Generalization Need | Priority |
|-------|---------------------|---------------------|----------|
| ProjectManager | Software development workflows | High - coordination patterns are universal | **P0** |
| Reviewer | Code review specifically | Medium - can extend to content review | **P1** |
| Planner | Implementation planning for code | Medium - planning is universal, examples are code-specific | **P1** |
| Coder | Pure code generation | Low - core competency should remain specialized | P2 |
| Writer | Documentation creation | Low - already reasonably general | P2 |
| Explore | Filesystem exploration | Low - domain-agnostic by design | N/A |
| Researcher | Web research | Low - already domain-neutral | N/A |
| Summarizer | Content summarization | Low - already flexible | N/A |

### Current Prompt Patterns (Anti-patterns Identified)

#### 1. Domain-Specific Language

**ProjectManager** currently states:
> "You are a PROJECT MANAGER. You own outcomes end-to-end: scoping work with the user, planning, assembling agent teams, tracking tasks, and ensuring quality delivery."

This frames the agent as specifically for project management in software contexts.

#### 2. Code-Only Examples

**Planner** examples include:
> "Plan migration from SQLite to PostgreSQL - 50GB data, 1hr downtime tolerance, using sqlx"

No non-coding planning scenarios are illustrated.

#### 3. Specialized Workflows Tied to Coding

**Reviewer** output expectations:
> "List findings grouped by severity... For each issue: location, problem, WHY it matters, suggested fix"

This is perfectly correct for code review but doesn't generalize to other review types (content, design, documentation).

---

## Before/After Prompt Updates

### Project Manager Agent (`project_manager.rs`)

#### Change 1: Role Definition Generalization

**BEFORE (lines 9):**
```
You are a PROJECT MANAGER. You own outcomes end-end: scoping work with the user, planning, assembling agent teams, tracking tasks, and ensuring quality delivery.
```

**AFTER:**
```
You are an ORCHESTRATION AGENT. You own outcomes end-to-end: scoping requirements, planning workflows, coordinating specialized agents, tracking progress, and ensuring quality delivery.

You apply these coordination principles regardless of domain—whether building software features, analyzing data sets, creating content pipelines, or managing system operations.
```

**Rationale:**
- "Orchestration" is domain-neutral while preserving the core coordination function
- Added explicit statement about cross-domain applicability
- Maintains all workflow mechanics unchanged

#### Change 2: Example Task Graph Generalization

**BEFORE (lines 34-43):**
```
Example task graph for "add authentication":
Task 1: Explore current auth setup (explore) — no deps
Task 2: Research JWT best practices (researcher) — no deps
Task 3: Implement auth middleware (coder) — blocked_by: [1, 2]
...
```

**AFTER:**
```
Example task graph for "add authentication" (coding domain):
Task 1: Explore current auth setup (explore) — no deps
Task 2: Research JWT best practices (researcher) — no deps
Task 3: Implement auth middleware (coder) — blocked_by: [1, 2]
...

Example task graph for "create quarterly report" (content domain):
Task 1: Gather data sources (explore) — no deps
Task 2: Research industry benchmarks (researcher) — no deps  
Task 3: Write analysis section (writer) — blocked_by: [1, 2]
Task 4: Create visualizations (coder) — blocked_by: [1]
Task 5: Review content quality (reviewer) — blocked_by: [3, 4]
```

**Rationale:**
- Shows the same coordination patterns applied to different domains
- Users see immediately that task graphs work universally
- Preserves original coding example for clarity and backward compatibility

#### Change 3: Agent Table Enhancement

**BEFORE (lines 81-91):** The agent table only shows coding use cases.

**AFTER:** Add domain examples to each agent entry:

```markdown
| Agent | Use When | Bash Access | Examples |
|-------|----------|-------------|----------|
| **explore** | Finding files, understanding project structure, searching filesystems | Read-only | "What config files exist?" (code), "Where are my data files?" (analysis), "Find all PDF reports" (content) |
| **researcher** | Needing web information, current events, external knowledge | None | "Best practices for X?" (any domain), "Current market trends" (business), "API documentation" (code) |
| **coder** | Writing/ modifying code, building scripts, data processing pipelines | Full (build, test with approval) | "Add validation to login" (app dev), "Create data transformation script" (analytics), "Automate file processing" (ops) |
...
```

**Rationale:**
- Demonstrates cross-domain applicability without changing agent capabilities
- Users see familiar patterns in their domain context
- No functional changes—purely illustrative

---

### Reviewer Agent (`reviewer.rs`)

#### Change 1: Role Definition Generalization

**BEFORE (lines 7-10):**
```
You are an autonomous code review agent. You receive CODE or FILE PATHS to review, along with optional focus areas.

## Your Mission
You provide thorough, actionable code reviews. Given a request like "Review src/auth.rs for security issues"...
```

**AFTER:**
```
You are an autonomous REVIEW AGENT. You receive CONTENT or FILE PATHS to review, along with optional focus areas and domain context.

## Your Mission
You provide thorough, actionable reviews adapted to the content type. Given a request like "Review src/auth.rs for security issues" (code) or "Review this documentation for clarity" (docs) or "Review this data pipeline for edge cases" (analytics), you autonomously analyze the content and provide structured feedback.
```

**Rationale:**
- "REVIEW AGENT" encompasses code, documentation, data, and other content types
- Examples show multiple domains while preserving original use case
- Core review methodology remains unchanged—only the scope expands

#### Change 2: Review Categories Adaptation Note

**BEFORE (lines 19-23):** Fixed categories for code.

**AFTER (add after line 23):**
```markdown
## Domain-Specific Review Focus

Adapt your review priorities based on content type:

**Code Review** (default): Security, bugs, performance, maintainability, style
**Documentation Review**: Clarity, completeness, accuracy, consistency, audience fit
**Data/Analytics Review**: Correctness, edge cases, performance, reproducibility
**Configuration Review**: Validity, security implications, best practices, documentation

When domain context isn't specified, ask: "What type of content is this and what aspects matter most?" This ensures your review focuses on what's relevant.
```

**Rationale:**
- Provides explicit guidance for different review types
- Maintains code review as default (backward compatibility)
- Encourages contextual questioning when domain is ambiguous

---

### Planner Agent (`planner.rs`)

#### Change 1: Mission Statement Generalization

**BEFORE (lines 7-10):**
```
You are an autonomous planning agent. You receive HIGH-LEVEL GOALS and produce detailed, actionable implementation plans.

## Your Mission
You create plans for tasks like "Migrate from SQLite to PostgreSQL" or "Add user authentication to the API". You break down complex goals into concrete steps that the available agents can execute.
```

**AFTER:**
```
You are an autonomous planning agent. You receive HIGH-LEVEL GOALS and produce detailed, actionable implementation plans.

## Your Mission
You create plans for diverse tasks across domains. Examples:
- **Software**: "Migrate from SQLite to PostgreSQL", "Add user authentication to the API"
- **Data/Analytics**: "Build quarterly sales analysis pipeline", "Create automated reporting system"
- **Content**: "Design content workflow for documentation overhaul", "Plan technical blog series"
- **Operations**: "Set up monitoring and alerting infrastructure", "Document incident response procedures"

You break down complex goals into concrete steps that leverage available agents and tools appropriately.
```

**Rationale:**
- Expands example scope while preserving original coding examples
- Shows planning patterns apply universally
- Maintains focus on agent coordination (core strength)

#### Change 2: Context Gathering Section Enhancement

**BEFORE (lines 35-45):** Focuses only on code exploration.

**AFTER (add to section, lines 43-45):**
```markdown
## ALWAYS Gather Context First

Before writing ANY plan, explore to understand the current state. Do NOT plan based on assumptions—discover them.

**For code projects:** Use Agent[explore] to find files, search for patterns, review git history.
**For data projects:** Explore data sources, understand schemas, check existing pipelines or scripts.
**For content projects:** Review existing documentation, understand audience and tone, inventory current assets.
**For operations:** Map current infrastructure, identify dependencies, review existing runbooks or procedures.

You have direct read access via `read_file`, `find_files`, `search_files`, and read-only bash commands. Use these for quick exploration. For deep dives into unfamiliar areas, delegate to Agent[explore].
```

**Rationale:**
- Provides domain-specific exploration guidance
- Keeps the same tools—just applies them differently per context
- Reinforces "discover don't assume" principle across domains

---

### Coder Agent (`coder.rs`)

#### Change: Example Domain Expansion (Minimal Update)

**BEFORE (lines 7-10):**
```
You are an autonomous coding agent. You receive HIGH-LEVEL GOALS about code to write or modify, not step-by-step instructions.

## Your Mission
You implement features like "Add input validation to the login form" or "Refactor the config module to support multiple profiles" by autonomously understanding context, planning, and writing code.
```

**AFTER:**
```
You are an autonomous coding agent. You receive HIGH-LEVEL GOALS about code, scripts, or automation to write or modify, not step-by-step instructions.

## Your Mission
You implement tasks like:
- **Application development**: "Add input validation to the login form", "Refactor the config module to support multiple profiles"
- **Data processing**: "Create a script to transform CSV data", "Build a pipeline to aggregate analytics"
- **Automation**: "Write a backup script for important directories", "Create deployment automation"

You accomplish these by autonomously understanding context, planning, and writing appropriate code.
```

**Rationale:**
- Minimal change—just expands examples beyond pure application development
- Scripts and automation are natural extensions of "coding"
- Maintains the same core mission and tools

---

### Writer Agent (`writer.rs`)

This agent is already well-generalized. **No changes required.**

The current prompt includes:
- Domain-agnostic mission statement ("create written content")
- Examples across documentation types (README, API docs, guides)
- Clear output destination handling
- Quality principles applicable to any writing

**Recommendation:** Consider adding one line to examples section showing non-documentation use cases:
```markdown
Examples:
  - 'Write a README for this project. Save to README.md'
  - 'Create API docs for src/api/users.rs. Return as response for review.'
  - 'Draft an email summarizing project status. Return as response.'
  - 'Write meeting notes template. Save to docs/meeting-template.md.'
```

---

## Implementation Roadmap

### Phase 1: Core Prompt Updates (Priority P0)

**Files to Modify:**
1. `crates/qq-agents/src/project_manager.rs` — Changes 1-3
2. `crates/qq-agents/src/reviewer.rs` — Changes 1-2
3. `crates/qq-agents/src/planner.rs` — Changes 1-2

**Steps:**
1. Create feature branch: `feature/prompt-generalization-core`
2. Apply Project Manager changes
3. Apply Reviewer changes  
4. Apply Planner changes
5. Run existing tests to verify no behavioral regression
6. Update documentation references if needed
7. Submit PR for review

**Estimated Effort:** 4-6 hours

**Acceptance Criteria:**
- All modified prompts compile without errors
- Existing agent tests pass
- New examples are syntactically correct in markdown
- No breaking changes to agent interfaces or capabilities

---

### Phase 2: Supporting Documentation (Priority P1)

**Files to Create/Modify:**
1. `docs/agent-framework.md` — Update with generalization concepts
2. `docs/examples/` — Add cross-domain usage examples

**Content for `docs/examples/cross-domain-workflows.md`:**
```markdown
# Cross-Domain Agent Workflows

## Data Analysis Pipeline

**User Request**: "Analyze quarterly sales data and create a report"

**Agent Coordination:**
1. PM scopes requirements (metrics, timeframe, output format)
2. Explore locates data files (`find_files "*.csv"`)
3. Researcher benchmarks industry standards
4. Coder builds analysis scripts and visualizations
5. Writer creates report narrative
6. Reviewer validates both code and content

## Documentation Overhaul

**User Request**: "Refactor our API documentation"

**Agent Coordination:**
1. PM defines scope (endpoints, audience, style guide)
2. Explore inventory current docs (`search_files "*.md"`)
3. Reviewer assesses current quality
4. Writer creates new documentation structure
5. Coder updates code comments to match
6. Reviewer validates consistency

## System Monitoring Setup

**User Request**: "Set up monitoring for our services"

**Agent Coordination:**
1. PM identifies services and metrics
2. Explore maps infrastructure (config files, deployment scripts)
3. Researcher researches best practices for chosen stack
4. Coder writes monitoring scripts and alerting rules
5. Writer documents runbooks
6. Reviewer validates configuration security
```

**Estimated Effort:** 3-4 hours

---

### Phase 3: Testing & Validation (Priority P1)

#### Test Categories

**1. Regression Testing (Existing)**
```bash
# Run all agent unit tests
cargo test -p qq-agents

# Verify prompt compilation
cargo check -p qq-agents
```

**2. New Integration Tests**
Create `tests/prompt_generalization.rs`:
```rust
#[test]
fn test_project_manager_examples_include_domains() {
    let pm = ProjectManagerAgent::new();
    let prompt = pm.system_prompt();
    
    // Verify coding example still present
    assert!(prompt.contains("add authentication"));
    
    // Verify content domain example added
    assert!(prompt.contains("create quarterly report") || 
            prompt.contains("content domain"));
}

#[test]
fn test_reviewer_handles_multiple_domains() {
    let reviewer = ReviewerAgent::new();
    let prompt = reviewer.system_prompt();
    
    // Verify non-code review examples present
    assert!(prompt.contains("documentation") || 
            prompt.contains("data pipeline"));
}

#[test]
fn test_planner_examples_span_domains() {
    let planner = PlannerAgent::new();
    let prompt = planner.system_prompt();
    
    // Check for multi-domain examples
    let has_code_example = prompt.contains("SQLite");
    let has_non_code = prompt.contains("analytics") || 
                       prompt.contains("operations") ||
                       prompt.contains("content");
    
    assert!(has_code_example);
    assert!(has_non_code);
}
```

**3. Manual Scenario Testing**
- Test PM with non-coding request: "Plan a marketing campaign launch"
- Test Reviewer with documentation: "Review this README for clarity"
- Test Planner with data project: "Design ETL pipeline for analytics"

**Estimated Effort:** 4-6 hours

---

## Testing Considerations

### Compatibility Matrix

| Agent | Coding Tasks | Data/Analytics | Content Creation | Operations |
|-------|--------------|----------------|------------------|------------|
| ProjectManager | ✅ Baseline | ✅ Should work | ✅ Should work | ✅ Should work |
| Reviewer | ✅ Baseline | ⚠️ Test edge cases | ✅ New capability | ⚠️ Test configs |
| Planner | ✅ Baseline | ✅ Planning universal | ✅ Planning universal | ✅ Planning universal |
| Coder | ✅ Baseline | ✅ Scripts OK | ❌ Not applicable | ✅ Automation OK |
| Writer | ✅ Baseline | ✅ Reports OK | ✅ Baseline | ✅ Documentation OK |

### Validation Approach

**Automated Tests:**
- Unit tests verify prompt content changes
- Integration tests verify agent coordination unchanged
- No behavioral regression in existing workflows

**Manual Testing:**
- Cross-domain scenario walkthroughs (5-10 scenarios)
- User acceptance with sample tasks from each domain
- Reviewer feedback on clarity and effectiveness

**Metrics to Track:**
- Task success rate by domain (target: ≥85% across all)
- Agent handoff quality (measure re-delegation frequency)
- User satisfaction scores (if available)

### Risk Mitigation

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Coding task effectiveness reduced | Low | High | Preserve all original examples; add domain sections rather than replace |
| Agent role confusion increases | Medium | Medium | Clear headers and domain-specific callouts in prompts |
| Reviewer over-generalizes | Medium | Medium | Keep code review as default; require explicit context for other types |
| Users don't recognize benefits | High | Low | Document changes prominently; provide migration guide if needed |

---

## Prompt Anti-Patterns to Preserve Against

### 1. Over-Generalization

**Anti-pattern:** Removing all domain-specific content until prompts are vague platitudes.

**Guardrail:** Always include at least one concrete example per major use case. The Project Manager should still show code examples—it should add, not replace.

### 2. Domain Confusion

**Anti-pattern:** Mixing domains within single examples so users don't see clear patterns.

**Guardrail:** Keep examples domain-consistent. Don't say "Research JWT and market trends"—separate them into distinct domain-specific examples.

### 3. Capability Creep

**Anti-pattern:** Implying agents can do things they can't (e.g., "Reviewer can fix bugs").

**Guardrail:** Explicitly state limitations in every prompt. Reviewer still cannot write files—only review.

### 4. Hidden Assumptions

**Anti-pattern:** Assuming all users have same context or domain knowledge.

**Guardrail:** Include "ask clarifying questions" guidance where domain is ambiguous.

---

## Maintenance Guidelines

### Prompt Update Protocol

When modifying agent prompts in the future:

1. **Preserve existing examples** — Add new ones, don't replace
2. **Document rationale** — Comment why changes were made near modified sections
3. **Test cross-domain impact** — Verify other domains aren't broken
4. **Update acceptance criteria** — Ensure tests cover new scenarios
5. **Version the prompts** — Consider adding prompt version identifiers for rollback

### Versioning Strategy

Consider adding to each agent file:
```rust
/// Prompt version: 2026-03-09 (generalization update)
/// Previous: 2024-XX-XX (initial coding-focused prompts)
const SYSTEM_PROMPT: &str = r#"...
```

This enables tracking prompt evolution and rolling back if needed.

---

## Appendix A: Complete Diff Summary

### File-Level Changes Required

| File | Lines Changed | Type | Description |
|------|---------------|------|-------------|
| `project_manager.rs` | ~15-20 lines modified/added | Core prompt | Role definition, examples, agent table |
| `reviewer.rs` | ~20-25 lines added | Core prompt | Domain adaptation section, mission statement |
| `planner.rs` | ~15-20 lines added | Core prompt | Multi-domain examples, context gathering |
| `coder.rs` | ~5-10 lines modified | Minimal | Expanded examples only |
| `writer.rs` | Optional 3-5 lines | Optional | Additional non-docs examples |

### New Files Required

| File | Purpose | Priority |
|------|---------|----------|
| `docs/examples/cross-domain-workflows.md` | Illustrate multi-domain usage | P1 |
| `tests/prompt_generalization.rs` | Validate prompt content changes | P1 |

---

## Appendix B: Sample User Scenarios Post-Generalization

### Scenario 1: Data Pipeline Development

**User:** "Build an ETL pipeline to process customer transaction data"

**Expected Agent Behavior:**
- PM orchestrates with domain-neutral coordination (unchanged)
- Explore finds existing data files and schemas
- Researcher benchmarks ETL patterns for the technology stack
- Coder writes transformation scripts and validation logic
- Reviewer validates code quality and edge case handling
- Writer documents pipeline architecture and usage

### Scenario 2: Content Campaign Planning

**User:** "Plan a technical blog series on Rust concurrency"

**Expected Agent Behavior:**
- PM scopes audience, timeline, topic coverage
- Explore checks existing content inventory
- Researcher validates topic accuracy and current best practices
- Writer drafts article outlines and content
- Reviewer assesses technical accuracy and clarity
- Coder creates code examples for each article (as needed)

### Scenario 3: Infrastructure Documentation

**User:** "Document our Kubernetes deployment setup"

**Expected Agent Behavior:**
- PM defines documentation scope and audience
- Explore maps config files, manifests, Helm charts
- Reviewer assesses current configuration quality
- Writer creates architecture docs and runbooks
- Coder updates inline comments to match documentation
- Reviewer validates consistency across all content

---

## Sign-off Sections

### Reviewers Required

- [ ] Framework Architect — Validate technical approach
- [ ] Documentation Lead — Verify documentation completeness
- [ ] Product Owner — Confirm user value proposition
- [ ] QA Engineer — Approve testing strategy

### Approval History

| Date | Version | Author | Changes | Approved By |
|------|---------|--------|---------|-------------|
| 2026-03-09 | 0.1 | System | Initial draft | Pending |

---

## Next Steps

1. **Immediate:** Assign owner for Phase 1 implementation
2. **Week 1:** Complete core prompt updates (ProjectManager, Reviewer, Planner)
3. **Week 2:** Run regression tests, gather initial feedback
4. **Week 3:** Complete documentation and integration tests
5. **Week 4:** Final validation, merge to main branch

**Target Completion:** 2026-03-30

---

*This document should be updated as implementation progresses to track decisions made, issues encountered, and lessons learned.*
