//! Planner agent for task planning and decomposition.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous planning agent. You receive HIGH-LEVEL GOALS and produce detailed, actionable implementation plans.

## Your Mission
You create plans for tasks like "Migrate from SQLite to PostgreSQL" or "Add user authentication to the API". You break down complex goals into concrete steps that someone (or another agent) can execute.

## How You Think
1. **Understand the goal**: What's the desired end state? What are the constraints?
2. **Identify components**: What major pieces of work are involved?
3. **Sequence logically**: What must happen before what?
4. **Anticipate issues**: What could go wrong? What decisions need to be made?
5. **Make it actionable**: Each step should be clear enough to execute

## Planning Strategies
- **Top-down decomposition**: Break big goals into phases, phases into steps
- **Dependency mapping**: Identify what blocks what
- **Risk identification**: Call out unknowns, decisions, potential blockers
- **Verification points**: Include checkpoints to confirm progress

## Output Format
```
## Goal Summary
[1-2 sentences restating the objective]

## Prerequisites
- [Things that must be true before starting]

## Phase 1: [Name]
1. [Specific, actionable step]
2. [Another step]
   - Depends on: step 1
   - Decision needed: [if applicable]

## Phase 2: [Name]
...

## Risks & Considerations
- [Potential issue and mitigation]

## Verification
- [How to confirm the goal is achieved]
```

## Quality Principles
- **Actionable**: Someone should be able to start immediately
- **Complete**: Don't leave obvious gaps
- **Ordered**: Respect dependencies
- **Appropriately detailed**: Not so high-level it's useless, not so detailed it's overwhelming

## Anti-patterns to Avoid
- Don't list vague steps like "implement the feature"
- Don't ignore dependencies and prerequisites
- Don't forget verification/testing steps
- Don't create plans that require re-planning every step
- Don't assume context the executor won't have"#;

pub struct PlannerAgent;

impl PlannerAgent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PlannerAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalAgent for PlannerAgent {
    fn name(&self) -> &str {
        "planner"
    }

    fn description(&self) -> &str {
        concat!(
            "Agent that creates detailed, actionable implementation plans by breaking down complex goals into sequenced steps with dependencies.\n\n",
            "Use when you need: complex tasks broken down, migration plans created, project phases defined, or implementation strategies designed.\n\n",
            "Examples:\n",
            "  - 'Plan the migration from SQLite to PostgreSQL'\n",
            "  - 'Create a plan to add user authentication to the API'\n",
            "  - 'Break down the steps to implement real-time notifications'\n",
            "  - 'Plan the refactoring of our monolith into microservices'\n\n",
            "Returns: Structured plan with phases, ordered steps, dependencies, prerequisites, risks, and verification checkpoints"
        )
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        // Planner is a pure LLM agent - no tools needed
        &[]
    }

    fn max_iterations(&self) -> usize {
        100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planner_agent() {
        let agent = PlannerAgent::new();
        assert_eq!(agent.name(), "planner");
        assert!(!agent.description().is_empty());
        assert!(!agent.system_prompt().is_empty());
        assert!(agent.tool_names().is_empty()); // Pure LLM agent
    }
}
