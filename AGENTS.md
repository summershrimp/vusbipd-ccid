## Repository Working Rules

When an Agent completes any task that modifies files in this repository, the Agent must create a git commit before considering the task finished.

### Commit Message Requirements

- The commit title must follow this format:

  `type: short description`

- The commit body must include all of the following:

  1. The original user input given to the Agent.
  2. The Agent's analysis of that input.
  3. The Agent's plan for the work.
  4. A summary of how the Agent implemented the final changes.

### Commit Message Template

```text
type: short description

Original user input:
<paste the user's original request>

Analysis:
<brief analysis of the request, constraints, and assumptions>

Plan:
<brief implementation plan>

Implementation summary:
<brief summary of the actual changes made>
```
