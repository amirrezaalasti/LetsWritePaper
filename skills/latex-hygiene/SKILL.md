---
name: latex-hygiene
description: Conventions for clean, robust LaTeX sources — sectioning, labels and cross-references, booktabs tables, figures, siunitx, non-breaking spaces, and BibTeX hygiene. Use when writing or editing .tex or .bib files.
kind: style
sections: []
---

# LaTeX Hygiene

Clean sources compile reliably, survive template changes, and make diffs reviewable. Never change the
venue template's class or style files; work inside them.

## Sectioning and structure
- Use the template's `\section`, `\subsection`, `\paragraph`; avoid going deeper than `\subsubsection`.
- Large papers: one file per section, pulled in with `\input{sections/method}`. Keep the preamble in the main file.
- One sentence per line (or per clause) in the source: diffs and `edit_document` anchors become much cleaner.
- Do not use `\\` or `\newline` to end paragraphs; use a blank line. Avoid manual `\vspace` hacks except as a last resort.
- Define macros for repeated names (`\newcommand{\method}{\textsc{FooNet}\xspace}`) so renaming is one edit.

## Labels and references
- Label right after the thing being labeled: after `\section{...}`, after `\caption{...}` (inside the float), inside `equation`.
- Prefix by type: `sec:`, `fig:`, `tab:`, `eq:`, `alg:`, `app:`, `thm:`, `lem:`. Use descriptive, lowercase, hyphenated
  names: `fig:pipeline-overview`, not `fig:1`.
- Never hard-code numbers: not "Section 3" or "Table 2", always `\cref{sec:method}` / `\ref{...}`.
- With `cleveref`, use `\cref` mid-sentence and `\Cref` at sentence start; load it after `hyperref`.
- Without `cleveref`, write `Section~\ref{sec:method}`, `Eq.~\eqref{eq:loss}`.
- Do not rename existing labels unless you update every reference.

## Non-breaking spaces
- Tie references and citations to the preceding word: `Figure~\ref{...}`, `prior work~\cite{...}`, `et al.~\cite{...}`.
- Also between numbers and units if not using siunitx: `16~GB`.
- After abbreviations ending in a period mid-sentence, use `\ ` or `~` (`e.g.\ the`, `i.e.,`) to avoid sentence-end spacing.

## Figures
```latex
\begin{figure}[t]
  \centering
  \includegraphics[width=\linewidth]{figures/overview.pdf}
  \caption{Overview of the pipeline. Takeaway sentence here.}
  \label{fig:pipeline-overview}
\end{figure}
```
- Prefer vector formats (PDF) for plots; PNG only for raster images.
- Size with `\linewidth`/`\columnwidth`, never absolute widths that break on template change.
- Use `figure*` for two-column-spanning figures. Placement `[t]` or `[tb]`; avoid `[h!]` everywhere.
- Font size in figures should be close to caption size; never rely on readers zooming.

## Tables (booktabs)
- `\toprule`, `\midrule`, `\bottomrule`; no vertical rules; no `\hline`; use `\cmidrule(lr){2-4}` for grouped headers.
- Caption above the table (convention for most venues), label after the caption.
- Align numbers on the decimal point (siunitx `S` columns) and state metric direction in the header (Acc.~$\uparrow$).
- Keep consistent precision within a column.

## Numbers and units (siunitx)
- `\num{12345}`, `\SI{3.5}{\giga\byte}` (v2) or `\qty{3.5}{\giga\byte}` (v3), `\SIrange`/`\qtyrange`, `\percent`.
- Use one convention throughout; check which siunitx version the template allows.

## Math
- `equation`/`align` for displays; never `$$...$$` or `eqnarray`.
- `\text{}` or `\operatorname{}` for words and named operators in math; `\mathrm{d}` or a macro for differentials if the venue prefers.
- `\left(`/`\right)` only when needed; consistent macro for vectors (`\vec`, `\mathbf`, or `\bm`).
- Punctuate display equations.

## BibTeX hygiene
- Consistent citation keys, e.g. `lastnameYYYYfirstword` (`vaswani2017attention`). Never duplicate keys; deduplicate
  entries that refer to the same paper.
- Protect capitalization in titles with braces: `title = {{BERT}: Pre-training of Deep {Transformers} for Language Understanding}`.
- Include DOI (or URL for preprints) for every entry where one exists; prefer published versions over arXiv when they exist.
- Correct entry types: `@inproceedings` with `booktitle` for conference papers, `@article` with `journal`, volume, pages.
- Use consistent venue names (full or abbreviated, not mixed).
- Escape special characters (`{\"u}`, `\&`) or ensure the toolchain handles UTF-8 (biber/biblatex).
- `\cite` variants: `\citet` for textual ("Smith et al. (2020) show"), `\citep` for parenthetical, when using natbib.

## Common errors to check
- Undefined references (`??`) or citations, multiply-defined labels, overfull hboxes in the log.
- `\label` before `\caption` (gives the section number instead of the figure number).
- Straight quotes `"..."` instead of ``` ``...'' ```.
- Hyphen vs. en-dash vs. em-dash: `-` for compounds, `--` for ranges (`1--5`), `---` for dashes.

## Working in LetsWritePaper
- Read the relevant `.tex` and `.bib` files with `read_document` before editing.
- Prefer small targeted `edit_document {path, old_text, new_text, rationale}` calls whose `old_text` is an exact, unique excerpt.
- Never invent BibTeX entries or keys; if a reference is missing, insert `TODO: cite ...` and tell the user.
