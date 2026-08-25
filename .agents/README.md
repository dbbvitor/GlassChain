# `.agents/` — agent working artifacts

Scratch space for anything an AI coding agent produces that is **not** shipped code.
Project rules live in [`../AGENTS.md`](../AGENTS.md); this folder holds the
work-in-progress around them.

```text
.agents/
├── handoff.md  # Session handoff — read this first when picking up the programme
├── plans/      # Implementation plans and specs — one file per effort
├── tasks/      # Active task breakdowns and checklists
└── memories/   # Durable findings worth carrying between sessions
```

## Rules

- Write a plan to `plans/<short-slug>.md` **before** starting any change that
  spans more than a couple of files, and implement against it.
- Record non-obvious discoveries in `memories/<topic>.md` — a subtle invariant, a
  footgun, why an approach failed. Save the next session the rediscovery.
- Keep files short and current. Delete or archive a plan once it ships; a stale
  plan is worse than no plan.
- If a memory turns out to be a durable project rule, **promote it into
  `AGENTS.md`** instead of leaving it buried here.
- **Never** put source code, secrets, credentials, `.pem` files, or generated
  build output in this folder.

## Templates

### `plans/<slug>.md`

```markdown
# <Title>

**Status:** draft | in progress | shipped
**Date:** YYYY-MM-DD

## Goal
One or two sentences. What is true when this is done?

## Context
What already exists, and which crates/files are involved.

## Approach
The chosen design, and the alternatives rejected (with the reason).

## Steps
- [ ] Step, with the file it touches
- [ ] ...

## Validation
Which commands prove this works: `cargo test --workspace`, a specific test name,
a manual two-node run, etc.

## Out of scope
What this deliberately does not do.
```

### `tasks/<slug>.md`

```markdown
# <Task>

**Plan:** ../plans/<slug>.md (if any)
**Status:** in progress | blocked | done

## Checklist
- [x] Done thing
- [ ] Next thing

## Notes
Decisions made mid-flight, blockers, open questions.
```

### `memories/<topic>.md`

```markdown
# <Topic>

**Learned:** YYYY-MM-DD

## Finding
The non-obvious thing, stated plainly.

## Evidence
Where it was observed — file, test, command output.

## Implication
What a future agent should do (or avoid) because of it.
```
