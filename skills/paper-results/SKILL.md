---
name: paper-results
description: Draft or revise results, experiments, evaluation, and ablation sections so every claim is backed by a table or figure, compared against fair baselines, and reported with appropriate statistics. Use when presenting experimental findings.
kind: writing
sections: ["result*", "experiment*", "evaluation", "ablation*"]
---

# Writing Results and Evaluation

Results are where claims become evidence. Reviewers read this section adversarially, looking for the
gap between what is shown and what is claimed.

## Organize by question, not by experiment
- Open with the research questions or hypotheses the evaluation answers (RQ1, RQ2, ...), ideally mirroring the
  contributions in the introduction.
- One subsection per question. Each opens with the answer in one sentence, then the evidence.
- Don't: "Table 2 shows the results on dataset A."
- Do: "Our method is most beneficial on long inputs: on A, the gap over the best baseline grows from 1.2 to 6.8
  points as length increases (Table 2)."

## Claims and evidence
- Every claim in prose must point to a specific table, figure, or appendix entry.
- Every table and figure must be referenced and interpreted in the text; tell the reader what to look at.
- Report the numbers that matter in the prose; do not make readers hunt in tables.
- Match the claim's scope to the evidence: three datasets support "on these benchmarks", not "in general".
- Avoid "significantly" unless a significance test was run; otherwise say "substantially" or give the number.

## Baselines
- Include the strongest relevant prior methods, a simple baseline (often surprisingly strong), and if applicable an
  oracle or upper bound.
- Give baselines the same tuning budget, data, and compute as your method; say so explicitly.
- If numbers are copied from prior papers, mark them and confirm the setup is identical.
- Explain any missing obvious baseline.

## Statistical reporting
- Report variability: mean ± std (or CI) over N seeds/runs, and state N. Single-run results need justification.
- State what the error bars mean (std, standard error, 95% CI) in the caption.
- For close comparisons, run an appropriate test (paired bootstrap, permutation test, Wilcoxon, t-test with
  stated assumptions) and correct for multiple comparisons when making many.
- Report effect sizes, not just p-values.
- Use consistent precision; do not report more digits than the variance justifies.
- Bold the best result only when the difference is meaningful; state the bolding rule in the caption.

## Ablations
- Ablate each component claimed to matter; one change at a time from the full model.
- Tie each ablation back to the design rationale in methods.
- Include sensitivity to key hyperparameters when the method introduces new ones.

## Honest negative results
- Report settings where the method does not help, and explain why if you can. Reviewers find them anyway, and
  stating them builds trust.
- Include an error analysis or qualitative examples of failures, not only cherry-picked successes; say how examples were selected.
- Report costs: training/inference time, memory, parameters, annotation effort.

## Tables and figures
- Captions are self-contained: what is shown, the metric and its direction (higher/lower is better), N runs, and the takeaway.
- Consistent method names and ordering across all tables and figures.
- Axes labeled with units; legible fonts; colorblind-safe palettes.

## Checklist
- [ ] Each RQ answered with a stated conclusion and pointed evidence.
- [ ] Numbers in text match tables exactly, and match the abstract and introduction.
- [ ] Variance and number of runs reported everywhere.
- [ ] Baselines tuned fairly and the procedure stated.
- [ ] Negative or mixed results reported.
- [ ] No test-set tuning.

## Working in LetsWritePaper
- Call `section_context {path, section}` before editing; check open comments tagged `experiments` or `soundness`.
- Prefer small targeted `edit_document {path, old_text, new_text, rationale}` calls with an exact, unique `old_text`.
- Use `propose_edit` for changes to claims or interpretations, or when the user asked for suggestions.
- Never fabricate or "fix" numbers. If a needed result is missing, insert `TODO: result for <setting>` and tell the user.
- Never invent citations; cite only keys present in the project's .bib files (check with `read_document`).
