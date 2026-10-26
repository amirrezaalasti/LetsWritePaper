---
name: paper-conclusion
description: Draft or revise a short conclusion that restates the problem and contributions, summarizes the evidence at the right strength, and ends with focused future work. Use when writing conclusion, summary, or future-work sections.
kind: writing
sections: ["conclusion*", "summary", "future work"]
---

# Writing the Conclusion

The conclusion is read by people who skipped to the end. It should leave them with the paper's claim,
the evidence for it, and what comes next — in a few short paragraphs.

## Structure (typically 1–3 paragraphs, under half a page)
1. **What was done.** One or two sentences restating the problem and the approach, phrased freshly rather than
   copied from the abstract.
2. **What was found.** The main findings, with the most important number or two, scoped as in the results.
3. **What it means.** The takeaway for the field or practitioners: what to do differently given these results.
4. **What is next.** One to three concrete future directions.

## Rules
- No new results, claims, figures, or citations that did not appear earlier.
- Claims must not be stronger than in the results and must respect the stated limitations.
- The contributions summarized must match the introduction's list in scope and wording strength.
- Avoid repeating the abstract verbatim; reviewers notice.
- Keep it short. If a venue counts it toward the page limit, it is the first place to trim.

## Future work
- Specific and motivated by a limitation or an observed result.
- Don't: "In future work, we plan to extend our approach to other domains."
- Do: "Because the gains vanish when labels are noisy (Section 5.3), combining our objective with noise-robust
  losses is a natural next step."
- Do not promise work you are not going to do as a way to deflect a limitation.
- If future work is a separate section, keep it to a few concrete directions, each tied to evidence.

## Endings to avoid
- "We hope this work will inspire future research in this exciting area."
- "In conclusion, our method opens up new possibilities."
- A list of generic platitudes about importance. End on the most concrete takeaway instead.

## Checklist
- [ ] Problem, approach, findings, implication, and next steps all present and brief.
- [ ] Numbers identical to the results section.
- [ ] Consistent with abstract and introduction contribution list.
- [ ] Nothing new introduced.
- [ ] Future work is concrete and grounded in the paper.

## Working in LetsWritePaper
- Call `section_context {path, section}` before editing; read the introduction and results with `read_document` to align claims.
- Prefer small targeted `edit_document {path, old_text, new_text, rationale}` calls with an exact, unique `old_text`.
- Use `propose_edit` when rewording claims or when the user asked for suggestions.
- Never invent citations; the conclusion rarely needs any. Cite only keys in the project's .bib files.
