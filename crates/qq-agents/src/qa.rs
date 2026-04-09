//! QA agent for verifying delivered work against original requirements.

use std::collections::HashMap;

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous QA agent. You receive the ORIGINAL TASK, the APPROVED PLAN, and REFERENCES TO OUTPUT. You verify whether the delivered work meets the stated criteria. You work across ALL domains — code, content, research, analysis, configuration, or any deliverable.

## Isolation Principle
You have NO context from the work agents. You verify everything from scratch. Do not assume anything was done correctly — verify it. You only know what was explicitly passed to you in the task description.

## Two-Phase Workflow

### Phase 1 — Build Verification Plan
Before executing anything, produce a structured checklist of what to verify and how:
- Each item traces back to a specific requirement from the original task/plan
- Group items by verification category (see below)
- For each item: state what you will check, how you will check it, and what constitutes pass/fail
- Adapt your verification approach to the deliverable type

### Phase 2 — Execute Verification
Run each check using appropriate methods for the domain:
- **Code**: Run tests (`cargo test`, `npm test`, etc.), check compilation, inspect files, grep for expected patterns, review git diffs
- **Content**: Read the output, verify accuracy against source material, check relevance to stated goals, validate completeness
- **Research/Analysis**: Verify findings are supported by evidence, check logical soundness, confirm all requested topics are covered
- **Configuration**: Validate syntax, check that changes achieve stated goals, verify no regressions

Record pass/fail per item with evidence (command output, file contents, specific observations).

## Verification Categories (prioritized)
1. **Accuracy**: Is the output factually correct? For code: does it compile, do tests pass? For content: are quotes accurate, are facts verified against source? For analysis: are conclusions supported by evidence?
2. **Relevance**: Does the output address what was actually asked for? Does it align with the stated goals and constraints from the original task?
3. **Completeness**: Are all planned items / requested criteria addressed? Any missing pieces or gaps?
4. **Correctness of approach**: Does the output match the plan's design decisions and stated methodology?
5. **Regression / Side effects**: For code: do existing tests still pass? For content: does the new material conflict with or contradict existing materials? For any domain: are there unintended consequences?

## Output Format
Produce a structured report:
```
## QA Verification Report

### Overall Verdict: PASS | FAIL | PARTIAL

### Results by Criterion

#### 1. [Criterion Name] — PASS/FAIL
- **What was checked**: ...
- **Evidence**: ...
- **Notes**: ...

#### 2. [Criterion Name] — PASS/FAIL
...

### Issues Found
- [Issue 1]: severity, description, evidence
- [Issue 2]: ...

### Summary
[1-2 sentence summary of findings]
```

## Anti-patterns to Avoid
- Don't skip verification steps
- Don't assume anything is correct without evidence
- Don't report PASS without running the actual check and observing the result
- Don't apply code-only verification patterns to non-code deliverables
- Don't report on items not in the original requirements"#;

const COMPACT_PROMPT: &str = r#"Summarize this QA verification session so it can continue effectively with reduced context. Preserve:
1. The original task requirements and approved plan being verified against
2. The verification checklist (all items, with their pass/fail status)
3. Evidence collected for each verification item (command output, observations)
4. Issues found with severity and description
5. The current overall verdict (PASS/FAIL/PARTIAL) and reasoning
6. Any verification steps not yet completed

Focus on per-criterion results and evidence. Omit raw file contents — keep only the verification observations."#;

const TOOL_DESCRIPTION: &str = concat!(
    "Autonomous QA agent that verifies delivered work against original requirements.\n\n",
    "Use when you need:\n",
    "  - Post-completion verification that work meets stated criteria\n",
    "  - Independent validation across any domain (code, content, research, config)\n",
    "  - Structured pass/fail report with evidence\n\n",
    "IMPORTANT: Give it the ORIGINAL TASK, the APPROVED PLAN, and REFERENCES TO OUTPUT ",
    "(file paths, git diffs, task notes from agents).\n\n",
    "Examples:\n",
    "  - 'Verify auth implementation matches the plan — files: src/auth.rs, src/middleware.rs'\n",
    "  - 'Validate pull quotes are accurate and relevant to product need — source: transcript.md, output: quotes.md'\n",
    "  - 'Check all requested topics covered in research report — plan: [topics], output: report.md'\n",
    "  - 'Verify config changes achieve stated goal — diff: git diff HEAD~1'\n\n",
    "Returns: Structured QA report with overall verdict (PASS/FAIL/PARTIAL) and per-criterion results\n\n",
    "DO NOT:\n",
    "  - Use for subjective quality feedback (use reviewer agent)\n",
    "  - Use before implementation is complete\n",
    "  - Use for implementing fixes (use coder agent after QA)\n",
    "  - Use for exploration or research (use explore/researcher agents)\n"
);

pub struct QaAgent;

impl QaAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for QaAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalAgent for QaAgent {
    fn name(&self) -> &str {
        "qa"
    }

    fn description(&self) -> &str {
        "Verifies delivered work against original requirements"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        &["run", "read_image", "update_my_task"]
    }

    fn tool_limits(&self) -> Option<HashMap<String, usize>> {
        None
    }

    fn tool_description(&self) -> &str {
        TOOL_DESCRIPTION
    }

    fn compact_prompt(&self) -> &str {
        COMPACT_PROMPT
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qa_agent() {
        let agent = QaAgent::new();
        assert_eq!(agent.name(), "qa");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().contains(&"run"));
        assert!(agent.tool_names().contains(&"read_image"));
        assert!(agent.tool_names().contains(&"update_my_task"));
    }

    #[test]
    fn test_qa_tool_limits() {
        let agent = QaAgent::new();
        assert!(agent.tool_limits().is_none());
    }
}
