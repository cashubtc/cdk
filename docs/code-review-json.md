# Automated Review JSON Format

Use this format when JSON review output is requested and the caller does not
provide a different schema. Apply the [CDK Code Review Guidelines](code-review.md)
for review scope, severity, and verdicts. Return valid JSON and nothing else.

Keep the existing object structure and enum values for compatibility with review
consumers. Do not add fields without coordinating with the consumer.

Example of a review with no findings:

```json
{
  "verdict": "APPROVE",
  "reason": null,
  "inline_comments": []
}
```

## Fields

- `verdict`: `APPROVE`, `COMMENT`, or `CHANGES_REQUESTED`, using the shared guide's
  definitions. These are descriptive verdicts, not authorization to submit a
  review. A GitHub submission adapter must map `CHANGES_REQUESTED` to the
  `REQUEST_CHANGES` event.
- `reason`: `null` when the inline comments fully explain the verdict and there
  is no additional material uncertainty. Otherwise, a concise string explaining
  the remaining concern, including findings that cannot be attached inline.
  Include their severity and actual file/line references when available. Do not
  repeat findings already explained inline.
- `inline_comments`: an array of objects with exactly these fields:
  - `path`: repository-relative path accepted by the current PR diff.
  - `line`: integer line number on the selected side of the diff.
  - `side`: `RIGHT` for the new version, or `LEFT` for the old version. Verify the
    side and anchor rather than guessing.
  - `severity`: `critical`, `warning`, or `nit`, assessed by impact and
    reachability as described in the shared guide.
  - `body`: a concise explanation of the triggering condition and consequence,
    with a concrete suggested fix when useful.

Use `reason` for unanchored findings instead of inventing inline coordinates or
dropping a finding about an unchanged consumer. Unanchored blockers still require
`CHANGES_REQUESTED`; an empty `inline_comments` array does not imply approval.
Consumers should display `reason` for any verdict, including `CHANGES_REQUESTED`.
This extends the previous guidance that used `reason` only for `COMMENT`, without
changing its nullable-string type or the JSON structure.

Keep private security details out of this output when it will be posted publicly;
follow [SECURITY.md](../SECURITY.md).
