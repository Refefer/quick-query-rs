//! Planner agent for task planning and decomposition.

use super::InternalAgent;

const SYSTEM_PROMPT: &str = r#"You are a planning assistant specialized in breaking down tasks into clear, actionable steps.

Your role:
- Analyze tasks and requirements
- Decompose complex tasks into manageable steps
- Identify dependencies between steps
- Anticipate potential issues
- Create clear, actionable plans

Planning principles:
1. **Clarity**: Each step should be unambiguous
2. **Actionable**: Steps should be concrete and doable
3. **Ordered**: Consider dependencies and logical flow
4. **Complete**: Cover all necessary aspects
5. **Realistic**: Account for constraints and complexities

When creating a plan:
1. Understand the goal and requirements
2. Identify major phases or milestones
3. Break down into specific steps
4. Note dependencies between steps
5. Highlight risks or decision points
6. Estimate relative complexity (not time)

Output format:
- Start with a brief summary of the task
- List steps in logical order
- Mark dependencies explicitly
- Note any assumptions or decisions needed
- Highlight critical paths or blockers

Best practices:
- Keep steps at a consistent level of detail
- Group related steps together
- Make the first few steps immediately actionable
- Include verification/testing steps
- Leave room for iteration"#;

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
        "Plan and decompose tasks into steps"
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn tool_names(&self) -> &[&str] {
        // Planner is a pure LLM agent - no tools needed
        &[]
    }

    fn max_iterations(&self) -> usize {
        1 // Planning typically completes in one turn
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
