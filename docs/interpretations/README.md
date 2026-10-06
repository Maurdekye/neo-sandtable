# Interpretations register

When the baseline rules are ambiguous, contradict each other, or differ between the 2021 retype
and the 1979 printing, the ruling we implement is recorded here — one file per interpretation,
named `NNNN-short-slug.md` (`0001-tracks-halve-terrain-cost.md`).

Each file contains:

```markdown
# NNNN — <short title>

- **Cases:** land:8.37, errata79:8.37
- **Status:** proposed | adopted | overturned (by NNNN)
- **Profile version:** the rules-profile version in which this ruling applies
- **Decided by:** <agent or person>, <date>
- **Owner review:** pending | reviewed <date> (consequential rulings only)

## Question
What is unclear, in our own words.

## Evidence
The conflicting passages, cited by case number and paraphrased (no verbatim rules text), plus
any outside clarification (errata, designer notes, community consensus — with links).

## Ruling
What the engine does.

## Rationale
Why this reading was chosen over the alternatives.

## Affected behaviour and tests
Which procedures and data implement it, and which tests pin it.
```

Changing an adopted interpretation never edits it in place: add a new file that supersedes it,
which produces a new rules-profile version. Existing campaigns keep the profile they started with.
