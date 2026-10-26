---
name: paper-methods
description: Draft or revise methods, approach, model, system-design, and experimental-setup sections so a skilled reader can understand and reproduce the work, with disciplined notation and explicit assumptions. Use when writing how the work was done.
kind: writing
sections: ["method*", "approach", "model", "system*", "design", "experimental setup", "setup"]
---

# Writing Methods and Setup

The test for a methods section: could a competent researcher in the area reimplement the approach and
rerun the experiments from the paper plus appendix, without emailing you?

## Structure
1. **Problem formulation.** Inputs, outputs, objective, and the setting (supervised? online? adversarial?).
   Define everything the rest of the section uses.
2. **Overview.** One paragraph and ideally a figure showing the components and data flow before any detail.
3. **Components.** One subsection per component, each starting with what it does and why it is needed,
   then how.
4. **Training / inference / optimization** procedure, including any algorithm box.
5. **Experimental setup** (if not a separate section): datasets, splits, preprocessing, baselines, metrics,
   hyperparameters and how they were chosen, compute budget, implementation details.

## Motivate design choices
- For each non-obvious choice, give one sentence of rationale and, where possible, point to the ablation that
  supports it. "We use X" without "because Y" invites a reviewer question.
- Separate what is essential to the idea from engineering choices that could be swapped.

## Notation discipline
- Define every symbol at first use, before or immediately after the equation.
- One symbol, one meaning, for the whole paper (including appendix and figures).
- Conventions: scalars italic lowercase, vectors bold lowercase, matrices bold uppercase, sets calligraphic — or
  whatever the field uses, applied consistently.
- Avoid symbols you never use again; name things in words if referenced only once.
- Number only equations you refer to.
- Punctuate display equations as part of the sentence.
- Consider a notation table in the appendix if more than ~15 symbols.

## Assumptions
- State assumptions explicitly, in one place, and label them (A1, A2) if theory depends on them.
- Distinguish modeling assumptions (what you believe about the world) from simplifications (what you chose for tractability).
- Say when an assumption is likely violated in practice and point to where that is discussed.

## Algorithms
- Use an algorithm block when there is an iterative or multi-step procedure that the prose would make hard to follow.
- Inputs and outputs at the top, consistent with the notation in the text, no undeclared variables.
- Keep it at the level of the idea; implementation trivia goes in the appendix or code.
- Reference it from the text and explain non-obvious lines.

## Reproducibility checklist
- [ ] Datasets: name, version, source, license, size, splits, and any filtering or preprocessing.
- [ ] Baselines: which implementations, which hyperparameters, tuned with equal budget as your method.
- [ ] Hyperparameters: final values and the search procedure/ranges; which split was used for selection.
- [ ] Metrics: exact definitions, especially if variants exist (macro vs. micro, normalized or not).
- [ ] Compute: hardware, runtime, number of runs/seeds.
- [ ] Software: key libraries and versions; code/data availability statement.
- [ ] For human studies: recruitment, compensation, consent/ethics approval, instructions given.

## Pitfalls
- Results or claims leaking into methods ("which significantly improves accuracy").
- Undefined symbols or symbols redefined later with a different meaning.
- "Standard preprocessing" or "default hyperparameters" without saying what they are.
- Test-set information used for any design decision.

## Working in LetsWritePaper
- Call `section_context {path, section}` before editing; check open comments tagged `reproducibility` or `soundness`.
- Prefer small targeted `edit_document {path, old_text, new_text, rationale}` calls with an exact, unique `old_text`.
  Copy equations into `old_text` verbatim; never retype math from memory.
- Use `propose_edit` for changes to formulations, equations, or assumptions, or when the user asked for suggestions.
- Never invent citations, dataset versions, or hyperparameter values. Cite only keys in the project's .bib files
  (check with `read_document`); mark unknowns as `TODO`.
