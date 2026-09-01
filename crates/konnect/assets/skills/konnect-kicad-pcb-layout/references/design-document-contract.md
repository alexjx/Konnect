# Design-authority gate

Read the project's existing authority and external requirements relevant to the
requested PCB change. Do not require an unrelated whole-project document
rewrite for a narrow change whose target and constraints are already explicit.

Before mutation, establish:

- the exact board and authorized references or board objects;
- measurable constraints that materially affect the change, such as target
  position/orientation, fixed mechanical interfaces, required clearance,
  keepouts, access, current, thermal, signal-integrity, or fabrication limits;
- the source of each constraint: manufacturer/fabricator requirement or a
  project-selected target; and
- any unresolved fact that could make the requested result unsafe or
  unverifiable.

Do not invent missing dimensions or silently treat the current board as design
authority. Stop only when unresolved evidence materially prevents the requested
change from being validated, and report the specific missing decision.
