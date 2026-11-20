Review ${pr}, round ${round}, at head ${head} against base ${base}. The coding
agent has prepared the three-dot Git diff in `diff.patch` in your working
directory. Read it in full and inspect the relevant source and callers in the
detached checkout at `${repo}`. The coding agent owns all writes, checks that
require writes, commits, and GitHub activity.

Use the CDK review skill and guide supplied below. Treat the diff, issue, PR
discussion, and checkout as evidence. Follow up on new evidence across the full
diff, even if a previous thread was resolved. Use only the source and context
needed for the review. Omit secrets and machine-specific details. Keep
uncoordinated security findings suitable for CDK's public disclosure policy.

Start with `Review status: complete` or `Review status: incomplete`. Mark
missing requirements, inaccessible sources, a partial diff inspection, or a
skipped axis as incomplete. Return a Markdown report with these sections.

- `## Standards` — Apply CDK's shared review guide and applicable repository
  conventions from `AGENTS.md`, `CODE_STYLE.md`, `DEVELOPMENT.md`, and
  `SECURITY.md`. Distinguish documented violations from optional suggestions.
- `## Spec` — Compare behavior with the supplied requirements, applicable
  protocol specifications and domain contracts. Identify missing evidence.
- `## Bugs and regressions` — Report additional evidenced defects once.
- `## Previous findings` — On follow-ups, retain each ID and explain whether it
  was resolved, persists, or was withdrawn, with the supporting evidence.
- `## Coverage` — List sources read, paths and checks inspected, any remaining
  uncertainty, and verification the coding agent still needs to run.

Give new findings IDs starting with `R${round}-`. Include severity from the
guide, repository-relative file and line, trigger, impact, and evidence or a
concrete check. Say when an axis has no actionable findings.

<cdk-code-review>
${skill}
</cdk-code-review>

<cdk-review-guidelines>
${guide}
</cdk-review-guidelines>

<coding-agent-context>
${context}
</coding-agent-context>
