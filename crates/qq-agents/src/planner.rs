//! Planner agent for task planning and decomposition.

use std::collections::HashMap;

use crate::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are an autonomous planning agent. You receive HIGH-LEVEL GOALS and produce detailed, actionable implementation plans.

## Your Mission
You create plans for tasks like "Migrate from SQLite to PostgreSQL" or "Add user authentication to the API". You break down complex goals into concrete steps that someone (or another agent) can execute.

Before you do any planning, make sure to use the available agents (as relevant) to gather context:
- **Agent[explore]**: Explore the filesystem to understand directory structure, find files, search for content
- **Agent[researcher]**: Research topics on the web when you need external information
- **Agent[reviewer]**: Review existing code to understand current implementation

Use these agents when you need to understand the current state before creating a plan.

## How You Think
1. **Gather context**: If planning involves existing systems, use Agent[explore] to understand current state
2. **Understand the goal**: What's the desired end state? What are the constraints?
3. **Identify components**: What major pieces of work are involved?
4. **Sequence logically**: What must happen before what?
5. **Anticipate issues**: What could go wrong? What decisions need to be made?
6. **Make it actionable**: Each step should be clear enough to execute

## Memory Tools
- `read_memory`: Check for existing plans before creating new ones
- `add_memory`: Persist finalized plans for later reference

Store plans with descriptive names like "migration-plan-postgres" or "auth-implementation-v2".

## Planning Strategies
- **Top-down decomposition**: Break big goals into phases, phases into steps
- **Dependency mapping**: Identify what blocks what
- **Risk identification**: Call out unknowns, decisions, potential blockers
- **Verification points**: Include checkpoints to confirm progress
- **Context gathering**: Use explore/researcher agents to understand current state before planning

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

## Keeping Users Informed
Use `inform_user` to notify the user about your progress WITHOUT ending your turn.
The user sees messages immediately while you continue working. This builds trust and transparency.

**When to use inform_user:**
- When gathering context: "Exploring the codebase to understand current architecture..."
- When delegating for info: "Asking explore agent to map out the module structure..."
- When you discover constraints: "Found that the app uses SQLite - this affects migration options..."
- When identifying risks: "Note: this will require database downtime - factoring into plan..."
- When structuring phases: "Breaking this into 4 phases to minimize risk..."
- When you find dependencies: "The auth system depends on 3 other modules - planning order carefully..."
- When the scope changes: "This is larger than expected - recommending a phased approach..."

**Examples:**
- inform_user({"message": "Analyzing the current authentication implementation..."})
- inform_user({"message": "Good news - existing tests cover 80% of affected code..."})
- inform_user({"message": "Identified a critical dependency - this must be updated first..."})

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

const TOOL_DESCRIPTION: &str = concat!(
    "Agent that creates detailed, actionable implementation plans by breaking down complex goals into sequenced steps.\n\n",
    "Use when you need:\n",
    "  - Complex tasks broken down into steps\n",
    "  - Migration plans created\n",
    "  - Project phases defined\n",
    "  - Implementation strategies designed\n\n",
    "IMPORTANT: Give it a GOAL and ask for a plan, not step-by-step instructions.\n\n",
    "Examples:\n",
    "  - 'Plan migration from SQLite to PostgreSQL - 50GB data, 1hr downtime tolerance, using sqlx'\n",
    "  - 'Plan adding OAuth2 auth to our API - Google/GitHub, 12 endpoints, currently no auth'\n\n",
    "Detailed example:\n",
    "  'Plan migrating our monolithic Django app to microservices. 150k LOC, PostgreSQL with 80 tables, ",
    "10k req/min peak. Constraints: max 5 min downtime, backwards compatibility for 6 months.'\n\n",
    "Returns: Structured plan with phases, ordered steps, dependencies, prerequisites, risks, and verification checkpoints\n\n",
    "DO NOT:\n",
    "  - Use for implementing code (use coder agent)\n",
    "  - Use for web research (use researcher agent)\n",
    "  - Use for simple tasks that don't need planning\n"
);

impl InternalAgent for PlannerAgent {
    fn name(&self) -> &str {
        "planner"
    }

    fn description(&self) -> &str {
        "Creates detailed implementation plans for complex tasks"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        // Planner uses memory tools for persisting plans; delegates to other agents for context
        &["read_memory", "add_memory"]
    }

    fn tool_limits(&self) -> Option<HashMap<String, usize>> {
        let mut limits = HashMap::new();
        limits.insert("read_memory".to_string(), 3);
        limits.insert("add_memory".to_string(), 3);
        Some(limits)
    }

    fn max_turns(&self) -> usize {
        100
    }

    fn tool_description(&self) -> &str {
        TOOL_DESCRIPTION
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
        assert!(agent.tool_names().contains(&"read_memory"));
        assert!(agent.tool_names().contains(&"add_memory"));
    }

    #[test]
    fn test_planner_tool_limits() {
        let agent = PlannerAgent::new();
        let limits = agent.tool_limits().expect("planner should have tool limits");
        assert_eq!(limits.get("read_memory"), Some(&3));
        assert_eq!(limits.get("add_memory"), Some(&3));
    }
}
