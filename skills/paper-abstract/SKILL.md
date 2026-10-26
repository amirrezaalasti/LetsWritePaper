---
name: paper-abstract
description: Draft or revise a paper abstract as a self-contained context-gap-approach-results-implication summary within the venue's length limit. Use when writing, shortening, or checking an abstract.
kind: writing
sections: ["abstract"]
---

# Writing the Abstract

The abstract is the only part most readers see. It must stand alone: a reader who skips the paper should
know what problem you addressed, what you did, what you found, and why it matters.

## Structure (one or two sentences per move)
1. **Context** — the problem area and why it matters. One sentence, not a history lesson.
2. **Gap** — what is missing or broken in current approaches. Be specific enough that it could be disputed.
3. **Approach** — what you did and the key idea that distinguishes it. Name the method if the paper does.
4. **Results** — the headline findings with numbers and a comparison point (baseline, prior state of the art, or a cost).
5. **Implication** — what this enables, changes, or suggests. One sentence.

Optional: a short clause on code/data release if the venue values it.

## Length
- Check the venue limit and respect it strictly: many ML conferences cap around 200–250 words, ACL-style around 200,
  IEEE/ACM journals often 150–250, some require a structured abstract. Ask the user if unknown.
- One paragraph unless the venue requires structured headings (Background/Methods/Results/Conclusions).

## Rules
- No citations, no footnotes, no references to figures, tables, or sections.
- No undefined acronyms. Spell out at first use or avoid; widely known ones (e.g. GPU, CNN) are acceptable for the field's audience.
- No math beyond trivial symbols; abstracts are indexed as plain text.
- Every number must match the results section exactly (same metric, dataset, rounding).
- State the scope of evidence: "on four benchmarks", "in a user study with 24 participants".
- Use present tense for what the paper shows ("we show", "X outperforms"); past tense for what was done in a study ("we collected").

## Do / Don't
- Don't: "Large language models have attracted significant attention in recent years."
  Do: "Large language models fail on arithmetic over more than ten digits."
- Don't: "Our method achieves promising results."
  Do: "Our method reduces error by 31% relative to the strongest baseline while using half the memory."
- Don't: "We propose a novel framework" (novelty is for reviewers to judge; the word adds nothing).
  Do: "We propose X, which replaces Y with Z so that ..."
- Don't end with a vague future-work sentence. End with the implication.

## Revision checklist
- [ ] Can each of the five moves be pointed to?
- [ ] Is at least one result quantitative?
- [ ] Do claims match the strength of evidence in the results and the scope stated in limitations?
- [ ] Is it consistent with the introduction's contribution list and the conclusion?
- [ ] Within the word limit (count it)?
- [ ] Would a researcher in an adjacent subfield understand every term?

## Working in LetsWritePaper
- Call `section_context {path, section: "abstract"}` first; also read the results and introduction with
  `read_document` so numbers and claims match.
- Prefer a targeted `edit_document {path, old_text, new_text, rationale}` with an exact, unique `old_text`; an abstract
  rewrite can replace the abstract body but never the surrounding file.
- Use `propose_edit` when changing the framing substantially or when the user asked for suggestions.
- The abstract has no citations; if a claim needs one, it belongs in the introduction.
