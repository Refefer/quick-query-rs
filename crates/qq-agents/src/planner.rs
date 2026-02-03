//! Planner agent for task planning and decomposition.

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
    "Agent that creates detailed, actionable implementation plans by breaking down complex goals into sequenced steps with dependencies.\n\n",
    "Use when you need: complex tasks broken down, migration plans created, project phases defined, or implementation strategies designed.\n\n",
    "IMPORTANT: Give it a GOAL and ask for a plan, not step-by-step instructions.\n\n",
    "Examples with context:\n",
    "  - 'Plan migration from SQLite to PostgreSQL - we have 50GB of data, can tolerate 1hr downtime, using Rust with sqlx'\n",
    "  - 'Plan adding auth to our API - we need OAuth2 with Google/GitHub, have 12 endpoints, currently no auth at all'\n\n",
    "Detailed example:\n",
    "  'Create a plan to migrate our monolithic Django app to microservices. Current state: 150k LOC Python, PostgreSQL ",
    "database with 80 tables, serves 10k requests/minute peak, deployed on AWS ECS. Team: 6 backend engineers, 2 DevOps. ",
    "Constraints: cannot have more than 5 minutes downtime, must maintain backwards compatibility with mobile apps for ",
    "6 months, budget for infrastructure is flexible but need to justify costs. We want to start by extracting the ",
    "user authentication service since it is the most stable and well-tested. Future services will be: payments, ",
    "notifications, search, and content. Plan should cover: service boundaries, data migration strategy, API gateway ",
    "setup, inter-service communication (we are leaning toward gRPC), observability, and rollback procedures.'\n\n",
    "Returns: Structured plan with phases, ordered steps, dependencies, prerequisites, risks, and verification checkpoints"
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
        // Planner doesn't need base tools - it uses other agents for information gathering
        &[]
    }

    fn max_iterations(&self) -> usize {
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
        assert!(agent.tool_names().is_empty()); // Uses agents, not base tools
    }
}
