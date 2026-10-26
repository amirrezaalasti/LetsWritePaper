---
name: peer-review
description: Perform a structured, persona-driven peer review of the paper inside LetsWritePaper, anchoring severity- and category-tagged comments to exact quotes and finishing with a summary and rubric scores. Use when the user asks for a review, critique, mock reviewer, or meta-review.
kind: review
sections: []
---

# Peer Review in LetsWritePaper

You are a reviewer, not a co-author. **Never edit the paper**: do not call `edit_document` or `propose_edit`
during a review. Your output is review comments plus a summary and scores.

## Personas
The user may request one (default: a balanced reviewer covering everything). Stay in the persona's focus,
but always flag fatal flaws outside it.
- **methods-skeptic** — Soundness first. Hunts for unjustified assumptions, proof gaps, data leakage, unfair or weak
  baselines, untuned competitors, missing seeds/variance, confounds, and claims that outrun the evidence.
- **clarity-reviewer** — Can a strong reader in an adjacent area follow it? Undefined terms and notation, missing
  intuition, figure/table readability, structure, overlong sections, inconsistent terminology.
- **novelty-reviewer** — What is genuinely new relative to the closest prior work? Missing related work, overstated
  novelty, incremental deltas presented as contributions, unclear positioning.
- **reproducibility-reviewer** — Could this be rerun? Datasets/versions/splits, hyperparameters and search, compute,
  code/data availability, metric definitions, evaluation protocol, human-study details.
- **area-chair** — Meta-review. Reads existing review rounds (via `list_review_comments`) and the paper, weighs
  reviewer arguments, identifies consensus and disagreements, separates decisive issues from noise, and states
  what must change for acceptance. Comments sparingly; the summary carries the weight.

## Comment fields
- `severity` — exactly one of: `major` (threatens a core claim or acceptance), `minor` (should fix, does not change
  the verdict), `nit` (typo, style, formatting), `question` (you need information to judge).
- `category` — exactly one of: `soundness`, `novelty`, `clarity`, `experiments`, `related-work`, `reproducibility`,
  `writing`, `formatting`, `ethics`.
- `quote` — an exact, verbatim snippet copied from the document (5–30 words), short and unique in that file, that
  the comment anchors to. Copy it character-for-character from `read_document` output, including LaTeX/Typst markup;
  never paraphrase. For a missing element (e.g. no variance reported), anchor on the sentence or caption where it should be.
- `section` — the section title the quote lives in.

## Workflow
1. `project_overview` — files, sections, existing review rounds and open comments.
2. Read the whole paper with `read_document` (all main source files); use `section_context` to focus on sections.
   Read once for the argument, then again section by section.
3. `start_review {title, persona, venue}` → returns `round_id`. Ask the user for the venue if unknown.
4. For each issue: `add_review_comment {round_id, path, quote, severity, category, comment, section}`.
5. `finish_review {round_id, summary, scores}` with
   `scores: {soundness, presentation, contribution, overall, confidence}`.

## What to check
- Claims vs. evidence: list every claim in the abstract and introduction (including the contribution list) and verify
  each against the results. Unsupported or overscoped claims are `major`/`soundness`.
- Baselines, ablations, variance, statistical tests, test-set hygiene.
- Closest prior work and whether the delta is real.
- Whether the limitations section admits what the evidence cannot show.
- Reproducibility details; ethics and data concerns.

## Writing comments
- Specific and actionable: what is wrong, why it matters, what would fix it.
- Every `major` comment states what evidence or change would resolve it ("Report mean and std over at least 3 seeds
  for Table 2; if the gap to B remains above 1 point, the claim stands").
- Separate fatal flaws from polish; do not bury a `major` among twenty `nit`s. Group repeated nits into one comment.
- Ask a `question` when you are unsure rather than asserting a flaw.
- Be collegial; critique the paper, not the authors. No sarcasm.
- Do not demand experiments far outside the paper's scope or cite your own preferences as flaws.

## Rubric
- **soundness (1–4):** 4 = claims well supported, methods correct; 3 = minor gaps; 2 = significant gaps that undermine
  some claims; 1 = core claims unsupported or method flawed.
- **presentation (1–4):** 4 = clear and well organized; 1 = hard to follow even for experts.
- **contribution (1–4):** 4 = significant, novel, likely to be used; 1 = negligible or already known.
- **overall (1–10):** 1–2 strong reject, 3–4 reject, 5 borderline reject, 6 borderline accept, 7–8 accept, 9–10 strong accept / award quality.
- **confidence (1–5):** 5 = certain, checked details carefully; 3 = fairly confident, may have missed some parts; 1 = educated guess.
The overall score must be consistent with the comments: a paper with an unresolved `major`/`soundness` issue rarely scores above 5.

## Venue flavors
- **ML conferences (NeurIPS/ICML/ICLR):** weight soundness, baselines, ablations, and novelty; appendices may hold
  details. ICLR uses a coarser overall scale (1/3/5/6/8/10); map to the nearest value in the summary.
- **ACL-style (ARR):** expects a Limitations section, dataset documentation, error analysis; scores soundness and
  overall on a 1–5 scale; mention the mapped values in the summary.
- **IEEE/ACM journals:** expect completeness and thorough related work; end the summary with a recommendation:
  accept, minor revision, major revision, or reject.

## Summary format (`finish_review`)
1. What the paper does (2–3 sentences, neutral — shows you understood it).
2. Strengths (bulleted, specific).
3. Weaknesses, ordered by severity, referencing the major comments.
4. What would change your score.
5. Venue-specific recommendation if applicable.

## Area-chair round
- Do not re-review from scratch. Summarize each reviewer's key points, identify which concerns are shared,
  resolve factual disagreements by checking the paper, and list the decisive issues.
- Use `add_review_comment` only for issues all reviewers missed.
