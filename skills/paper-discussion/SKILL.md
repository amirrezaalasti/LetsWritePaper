---
name: paper-discussion
description: Draft or revise discussion, limitations, and threats-to-validity sections that interpret findings, state their boundaries honestly, and address broader impact. Use when explaining what results mean, where they stop applying, and what risks they carry.
kind: writing
sections: ["discussion", "limitation*", "threats*"]
---

# Writing Discussion, Limitations, and Threats to Validity

Results say *what* happened; the discussion says *why*, *how far it generalizes*, and *what it does not show*.
A good limitations section pre-empts the reviewer's strongest objections in your own words.

## Interpretation
- Start from the most important finding and explain the mechanism you believe produces it. Label speculation
  as such ("one plausible explanation is ...") and say what evidence would confirm it.
- Connect findings back to the gap stated in the introduction: is it closed, narrowed, or reframed?
- Relate to prior work: where do your results agree with, extend, or contradict published findings? Offer reasons for disagreements.
- Discuss surprising or mixed results rather than hiding them.
- Do not restate the results section; reference it and add interpretation.

## Limitations
Write them as specific, bounded statements, each with its consequence and, if possible, a mitigation.
- Don't: "Our approach has some limitations and may not generalize to all settings."
- Do: "All experiments use English text; the tokenization-dependent component (Section 3.2) may behave differently
  for morphologically rich languages."
Cover, as applicable:
- Scope of data, domains, languages, populations, or scales tested.
- Assumptions that may not hold in practice (link to where they are stated in methods).
- Computational cost and who can afford to use the method.
- Evaluation limits: metric validity, benchmark contamination, small sample sizes, single runs.
- Things you did not compare against and why.
Check the venue: some (e.g. ACL-style) require a dedicated Limitations section that does not count toward the page limit.

## Threats to validity (empirical and software-engineering venues)
Organize by type:
- **Internal:** could something other than the treatment explain the result (confounds, bugs, leakage, tuning imbalance)?
- **External:** do findings transfer beyond the studied subjects, datasets, or settings?
- **Construct:** do the metrics measure the concept you claim (e.g. is BLEU a proxy for adequacy here)?
- **Conclusion:** are statistical conclusions sound (power, multiple comparisons, test assumptions)?
For each threat: state it, state what you did to mitigate it, state the residual risk.

## Broader impact and ethics
- Identify concrete, plausible misuse or harm, and who bears it; avoid both boilerplate and catastrophizing.
- Data: consent, licensing, privacy, demographic coverage, annotator treatment.
- Environmental and financial cost when large-scale compute is involved.
- Follow the venue's ethics/impact statement requirements (placement, length, checklist).

## Checklist
- [ ] Each major finding interpreted, with speculation labeled.
- [ ] Limitations are specific and consequential, not generic disclaimers.
- [ ] No limitation contradicts a claim made in the abstract or introduction (if it does, fix the claim).
- [ ] Threats to validity mapped to mitigations where relevant.
- [ ] Ethics/impact statement meets venue requirements.

## Working in LetsWritePaper
- Call `section_context {path, section}` before editing; open review comments often name limitations the paper should acknowledge.
- Prefer small targeted `edit_document {path, old_text, new_text, rationale}` calls with an exact, unique `old_text`.
- Use `propose_edit` when adding limitations that change how the paper's claims read, or when the user asked for suggestions.
- Never invent citations; cite only keys present in the project's .bib files (check with `read_document`), and mark
  missing ones as `TODO: cite ...`.
