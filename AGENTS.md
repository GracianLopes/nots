## Execution & Anti-Redundancy Rules

The purpose of these rules is to make development efficient, NOT to reduce the amount of work required to complete a task.

### 1. Use Existing Context

At the start of a session, read `/home/pluto/Desktop/notsAI/context.md` once.

Treat verified information in `context.md` as the current project state.

Do NOT repeatedly re-verify information already established there unless:
- the relevant files have changed,
- the information is explicitly marked uncertain,
- new evidence contradicts it,
- or the current task requires information that is not recorded.

### 2. Do Necessary Work Fully

Do not interpret the anti-redundancy rules as permission to skip implementation, investigation, testing, validation, error handling, or integration.

For every requested task:

1. Understand the goal.
2. Inspect the files and dependencies genuinely required.
3. Implement the complete requested functionality.
4. Integrate it with the existing system.
5. Run the appropriate verification gates.
6. Fix problems found during validation.
7. Continue until the requested task is complete.

Do not leave work partially implemented when the remaining steps are clear and can be completed.

### 3. No Redundant Verification

A successful tool result is authoritative for the current task.

Do not repeatedly:
- read the same file,
- check the same function,
- verify the same TODO,
- check the project root,
- check repository state,
- re-read `context.md`,
- or re-establish facts already known.

Before making a tool call, ask:

"Does this provide new information required for my next action?"

If not, do not make the call.

### 4. Keep the Plan Stable

Once a valid implementation plan has been established, follow it.

Do not restart planning or reconsider completed decisions after every tool call.

Only change the plan when new evidence materially changes the task.

If the next implementation step is clear, perform it instead of asking for confirmation.

### 5. Investigate When Actually Necessary

Do not avoid investigation merely to save tokens.

If understanding another file, dependency, API, architecture component, or existing implementation is necessary to correctly complete the task, inspect it.

The rule is:

**Do all investigation necessary to complete the task correctly, but never repeat investigation that has already produced the required answer.**

### 6. Execution Loop

Use this workflow:

READ → PLAN → IMPLEMENT → VALIDATE → FIX → VALIDATE → COMPLETE

Do not get stuck in:

READ → VERIFY → RE-VERIFY → RE-PLAN → RE-VERIFY

### 7. Context Maintenance

After meaningful codebase changes, update `context.md` so it reflects the new current state.

`context.md` is a project-state file, not a reasoning diary.

Record:
- completed functionality,
- current implementation state,
- important architecture decisions,
- known constraints,
- current next steps.

Do NOT record:
- chain-of-thought,
- tool-call narration,
- repeated verification,
- temporary uncertainty,
- or every intermediate reasoning step.

### 8. Completion

When the requested task is fully implemented and validated:

1. Update `context.md`.
2. Report what changed.
3. Report validation results.
4. Stop.

Do not continue exploring the repository after the task is complete.