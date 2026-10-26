---
name: academic-paper
description: Whole-paper planning for a research manuscript — pick the venue register, fix the argument spine, build an outline, decide the writing order, and keep claims consistent across abstract, introduction, and conclusion. Use when starting a paper, restructuring a draft, or deciding what to write next.
kind: writing
sections: []
---

# Planning the Whole Paper

A paper is one argument with supporting evidence, not a collection of sections. Settle the argument first;
the sections are just where its parts live.

## 1. Pin down the target venue
- Identify the venue (conference, journal, workshop, thesis chapter) and record its page limit, template,
  anonymity rules, required sections (limitations, ethics, broader impact, reproducibility checklist), and
  reference-format rules. Ask the user if unknown; do not guess.
- Match the register to the venue:
  - ML conferences (NeurIPS/ICML/ICLR): crisp claims, strong baselines, ablations, appendix for proofs and details.
  - ACL-style: explicit research questions, careful dataset description, error analysis, mandatory Limitations section.
  - IEEE/ACM journals: fuller related work, complete methodology, longer and more formal; reviewers expect completeness over punch.
  - Workshops: one sharp idea, preliminary evidence is acceptable if labeled as such.
- Budget pages per section before drafting (e.g. 8 pages: intro 1, related 0.75, method 2.5, results 2.5, discussion+conclusion 1, slack 0.25).

## 2. Write the argument spine (before any prose)
Produce five sentences and get the user to agree on them:
1. **Problem:** what matters and to whom.
2. **Gap:** what existing work cannot do, stated so it could be checked.
3. **Idea:** the one insight that makes your approach work.
4. **Evidence:** the strongest result, with a number and a comparison point.
5. **Consequence:** what the reader can now do or believe that they could not before.

Every section must serve one of these five sentences. Material that serves none goes to the appendix or is cut.

## 3. Build the outline
- One line per paragraph, phrased as the claim that paragraph makes ("Baselines degrade sharply beyond 10k tokens"),
  not a topic label ("Long context").
- Map each contribution to the section and the figure/table that substantiates it. A contribution with no
  evidence location is not a contribution yet.
- Plan figures early: Figure 1 should convey the core idea or headline result to someone who reads nothing else.

## 4. Writing order
Recommended order, because each step constrains the next:
1. Methods (you know it best; fixes notation and terminology).
2. Results (fixes what you can actually claim).
3. Introduction (now you know the true contributions).
4. Related work (positioning against the now-fixed contributions).
5. Discussion / limitations.
6. Conclusion.
7. Abstract last, distilled from the final intro and results.
8. Title after the abstract.

## 5. Cross-section consistency audit
Run this after any substantive change:
- [ ] Every number in the abstract and introduction appears identically in the results (same metric, dataset, decimals).
- [ ] The contribution list in the introduction matches the conclusion's summary — same items, same strength of claim.
- [ ] Claims are scoped identically everywhere ("on three benchmarks" in results is not "in general" in the abstract).
- [ ] Each technical term is defined once, before first use, and used with one meaning throughout.
- [ ] Notation is consistent across sections, figures, and appendix.
- [ ] Limitations acknowledged in the discussion are not contradicted by overclaiming elsewhere.
- [ ] Every figure and table is referenced in the text and discussed, not just displayed.

## 6. Routing to section skills
Use the section-level skills for drafting: `paper-abstract`, `paper-introduction`, `paper-related-work`,
`paper-methods`, `paper-results`, `paper-discussion`, `paper-conclusion`. Apply `clarity-editing` as a final pass,
and `latex-hygiene` or `typst-writing` depending on the source format.

## Common failure modes
- Starting with the introduction and then discovering the results do not support it.
- A related-work section longer than the method.
- "We propose X" with no sentence anywhere explaining why X should work.
- Results that answer a different question than the introduction asked.

## Working in LetsWritePaper
- Start with `project_overview` to see files, sections, and open review comments; read the full draft with `read_document`.
- Call `section_context {path, section}` before editing any section; it returns the text, applicable skills, and open comments.
- Prefer small, targeted `edit_document {path, old_text, new_text, rationale}` calls where `old_text` is an exact,
  unique excerpt. Do not rewrite whole files.
- Use `propose_edit` for structural reorganizations or when the user asked for suggestions rather than changes.
- Never invent citations. Only cite keys present in the project's .bib files (check with `read_document`);
  mark anything missing as `TODO: cite <description>`.
