---
name: paper-related-work
description: Draft or revise related-work and background sections as thematic synthesis that positions the paper fairly against prior research. Use when writing related work, background, or prior-work comparisons.
kind: writing
sections: ["related*", "background", "prior work"]
---

# Writing Related Work and Background

Related work answers two questions for the reviewer: *Do the authors know the field?* and *What exactly is
new here?* It is an argument about positioning, not an annotated bibliography.

## Background vs. related work
- **Background** teaches the reader what they need to follow the method: definitions, standard formulations,
  notation. It is neutral and short.
- **Related work** compares: it groups prior approaches and explains how this paper differs from each group.
- If both exist, do not repeat material between them.

## Organize by theme, not by paper
- Choose 2–5 themes that correspond to the axes on which your contribution differs (e.g. "supervision source",
  "model architecture", "evaluation setting").
- Use a bolded run-in heading or subsection per theme.
- Within a theme: state the shared idea, cite the representative works together, note the key variants, then
  close with one sentence on how your work relates.
- Don't: "Smith et al. proposed A. Jones et al. proposed B. Lee et al. proposed C."
- Do: "Several methods learn alignments from parallel text [a, b, c]; they differ mainly in how they handle
  unaligned tokens [b, d]. All of them assume parallel data at training time, which we do not."

## Positioning
- Every theme should end with a differentiating sentence. If you cannot write one, either the theme is irrelevant
  or your contribution overlaps — tell the user.
- Distinguish clearly between: (a) work you build on, (b) work you compete with (should appear as baselines or have
  a stated reason why not), (c) work that addresses a related but different problem.
- The closest prior work deserves the most precise treatment: state concretely what it does, what you do differently,
  and why the difference matters. Reviewers check this first.

## Fair characterization
- Describe prior work as its authors would recognize it. Do not set up straw men.
- Prefer factual differences ("requires labeled data", "assumes a fixed vocabulary") over value judgments
  ("naive", "fails to", "simplistic").
- Acknowledge strengths of competing approaches; it makes your positioning more credible.
- If you criticize a method, the criticism should be checkable, ideally by your experiments.
- Include recent and concurrent work; note concurrent work explicitly as such.

## Placement and length
- ML conferences: often after the introduction or before the conclusion; ~0.5–1 page.
- Journals: usually after the introduction, more comprehensive.
- Keep it proportional: it should not exceed the method section.

## Checklist
- [ ] Organized by theme, each theme ending in a positioning sentence.
- [ ] Closest prior work identified and contrasted precisely.
- [ ] Every competing method either appears as a baseline or has a stated reason for exclusion.
- [ ] No unsupported negative claims about prior work.
- [ ] No citation dumps of 8+ references without saying what they share.
- [ ] Consistent with the gap stated in the introduction.

## Working in LetsWritePaper
- Call `section_context {path, section}` before editing; it lists open review comments (often "missing related work").
- Prefer small targeted `edit_document {path, old_text, new_text, rationale}` calls with an exact, unique `old_text`.
- Use `propose_edit` when restructuring themes or when the user asked for suggestions.
- Never invent citations — this is the section where it is most tempting. Only cite keys present in the project's
  .bib files (check with `read_document`). For a paper you believe exists but is missing, write
  `TODO: cite <authors/title/year as remembered>` and tell the user to verify and add it.
