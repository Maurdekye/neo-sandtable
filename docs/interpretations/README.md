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

## A ruling must be on main before it goes to the owner

A consequential ruling enters an owner review batch only once its file is on `main`, even at
status `proposed` with no implementation behind it. A draft that lives only in an agent's scratch
folder cannot carry an owner review that anyone else can see: the batch gets approved, the
adoption pass finds no file to mark, and the agent waiting on the answer never learns it arrived.
That happened to `air-0018` in batch 4 and cost most of a day.

So: land the file first, then put it in the batch. When proposing candidates, state the `main` SHA
each file is on.
