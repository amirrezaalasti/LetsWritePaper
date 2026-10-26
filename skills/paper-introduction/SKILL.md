---
name: paper-introduction
description: Draft or revise an introduction that motivates the problem, states a precise gap, lists verifiable contributions, and previews the paper. Use when writing an introduction, opening paragraphs, or a contribution list.
kind: writing
sections: ["intro*"]
---

# Writing the Introduction

The introduction is a compressed version of the whole argument. Reviewers often decide their score
here and read the rest looking for confirmation.

## Paragraph plan (typically 4–6 paragraphs, about one page)
1. **Problem and stakes.** Open with the specific problem, not the field. Why does it matter, to whom,
   and what goes wrong today? A concrete example or failure case is stronger than a general statement.
2. **What exists and where it falls short.** Summarize the dominant approaches in two to four sentences with
   citations, then state the gap precisely: a limitation that is observable, not "has not been explored".
3. **Key insight / approach.** The one idea that makes your solution work, in plain language, before any details.
   Explain *why* it should address the gap.
4. **Evidence preview.** The headline results with numbers and comparison points.
5. **Contributions.** A short bulleted list (see below).
6. **Roadmap** (optional; skip if the venue is short and the structure is standard).

## Contribution list
Each item must be something a reviewer could verify by pointing to a section, theorem, table, or artifact.
- Don't: "We provide a comprehensive analysis of X."
- Do: "We show that X degrades by 40% under distribution shift on three benchmarks (Section 5.2)."
- Don't: "We propose a novel, efficient framework."
- Do: "We introduce Y, a training objective that removes the need for paired data (Section 3)."
- Typical items: a method, a theoretical result, a dataset/benchmark, an empirical finding, a released artifact.
- Three or four items; avoid splitting one contribution into several to pad the list.
- Contributions must match the conclusion and abstract exactly in scope and strength.

## Gap statement discipline
- A gap is a limitation with a consequence: "Existing retrievers require labeled queries, which are unavailable for
  low-resource languages."
- "No prior work has studied X" is weak on its own; say why studying X matters and what would change.
- Do not misrepresent prior work to manufacture a gap; reviewers often authored it.

## Roadmap paragraph
- One short paragraph mapping sections to content: "Section 2 reviews ..., Section 3 describes ..., Section 5 ...".
- Use cross-references, not hard-coded numbers.
- Skip boilerplate if the structure is entirely conventional and space is tight.

## Pitfalls
- Opening with "In recent years, X has gained increasing attention" — cut and start with the problem.
- A related-work survey inside the introduction; keep only what is needed to establish the gap.
- Introducing notation or method details that belong in Methods.
- Claims stronger than the results support ("solves", "guarantees", "state-of-the-art" without qualification).
- Forward references to results that never appear.

## Checklist
- [ ] Could a reader state the problem, the gap, and the key idea after the first page?
- [ ] Every claim cited or supported by the paper's own results.
- [ ] Every contribution points to where it is substantiated.
- [ ] Numbers match the results section.
- [ ] Terminology matches the methods section.

## Working in LetsWritePaper
- Call `section_context {path, section}` before editing; it returns the section, applicable skills, and open review comments.
- Read the results and conclusion with `read_document` to keep claims consistent.
- Make small targeted `edit_document {path, old_text, new_text, rationale}` calls with an exact, unique `old_text`;
  avoid rewriting the whole file.
- Use `propose_edit` for substantive reframing or when the user asked for suggestions.
- Never invent citations. Cite only keys that exist in the project's .bib files (check with `read_document`);
  otherwise write `TODO: cite <what is needed>`.
