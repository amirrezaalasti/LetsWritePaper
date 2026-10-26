---
name: typst-writing
description: Conventions for writing academic papers in Typst — headings, labels and references, figures, tables, bibliography from .bib files, math, set/show rules, and templates — with LaTeX equivalents. Use when writing or editing .typ files.
kind: style
sections: []
---

# Writing Papers in Typst

Typst is markup plus a scripting language. Most LaTeX habits translate directly; the main differences are
that references and citations share the `@` syntax, styling is done with `set`/`show` rules, and math has its own syntax.

## LaTeX to Typst quick map
| LaTeX | Typst |
|---|---|
| `\section{Intro}` / `\subsection{...}` | `= Intro` / `== ...` |
| `\label{sec:intro}` | `<sec:intro>` placed right after the heading/figure/equation |
| `\cref{sec:intro}` / `\cite{key}` | `@sec:intro` / `@key` |
| `\citet{key}` | `#cite(<key>, form: "prose")` |
| `\emph{x}` / `\textbf{x}` | `_x_` / `*x*` |
| `\footnote{...}` | `#footnote[...]` |
| `~` (non-breaking space) | `~` |
| `% comment` | `// comment` or `/* ... */` |

## Document setup (set rules)
```typst
#set document(title: [Paper Title], author: ("A. Author",))
#set text(size: 10pt, lang: "en")
#set par(justify: true)
#set heading(numbering: "1.1")              // required for @sec: references to headings
#set math.equation(numbering: "(1)")         // required for @eq: references
#show figure.where(kind: table): set figure.caption(position: top)
```
- If the venue provides a Typst template, use it: `#import "template.typ": paper` then `#show: paper.with(title: [...], authors: (...))`,
  or import a pinned version of a Typst Universe package (`#import "@preview/<name>:<version>": ...`). Never edit the template itself.
- Keep global `set`/`show` rules at the top of the main file, not scattered through sections.
- Split long papers with `#include "sections/method.typ"`.

## Labels and references
- Same prefix discipline as LaTeX: `sec:`, `fig:`, `tab:`, `eq:`, `app:`; descriptive lowercase names (`<fig:pipeline-overview>`).
- `@fig:pipeline-overview` renders the supplement and number ("Figure 2"); do not also type "Figure" before it.
  To change the supplement: `@sec:method[Sec.]`, or globally with `#set figure(supplement: ...)` / `#set heading(supplement: ...)`.
- Referencing a heading or equation fails to compile unless numbering is enabled for it.
- Never hard-code numbers ("Section 3").
- Appendix: `#counter(heading).update(0)` then `#set heading(numbering: "A.1")`.

## Figures
```typst
#figure(
  image("figures/overview.svg", width: 100%),
  caption: [Overview of the pipeline. Takeaway sentence here.],
) <fig:pipeline-overview>
```
- Prefer SVG or PDF for plots. Use `placement: top` (or `auto`) for floating; in two-column layouts use
  `scope: "parent"` for a full-width figure if the Typst version supports it.

## Tables (booktabs style)
```typst
#figure(
  table(
    columns: (auto, 1fr, 1fr),
    align: (left, right, right),
    stroke: none,
    table.hline(),
    table.header([Method], [Acc. ↑], [Time (s) ↓]),
    table.hline(stroke: 0.5pt),
    [Baseline], [71.2], [3.4],
    [Ours],     [*74.8*], [3.1],
    table.hline(),
  ),
  caption: [Main results (mean of 5 seeds).],
) <tab:main>
```
- No vertical lines; three horizontal rules; consistent precision per column.

## Bibliography
- `#bibliography("refs.bib", style: "ieee")` at the end (other built-in styles include `"apa"`, `"chicago-author-date"`,
  `"association-for-computing-machinery"`; a CSL file path also works). Multiple files: `#bibliography(("a.bib", "b.bib"))`.
- Cite with `@key` or `#cite(<key>)`; keys containing characters like `/` or `:` at the end may need `#cite(<key>)`.
- The same BibTeX hygiene applies as for LaTeX: consistent keys, DOIs, correct entry types, no duplicates.
  Title braces for capitalization are respected by Typst's BibTeX parser for most styles.

## Math
- Inline `$x^2$`; display has spaces inside the dollars: `$ L = sum_(i=1)^n ell(y_i, hat(y)_i) $ <eq:loss>`.
- Multi-letter words need quotes or operators: `"softmax"`, `op("argmax")`; `ab` alone is parsed as an identifier,
  write `a b` for a product.
- Grouping uses parentheses, not braces: `x_(i j)`, `e^(-t)`, `frac(a, b)` or `a / b`.
- Common: `bold(x)` for vectors, `cal(D)` for sets, `RR`, `NN`, `->`, `<=`, `!=`, `dots`, `mat(...)`, `vec(...)`, `cases(...)`.
- Alignment uses `&` and line breaks `\` inside one display.

## Pitfalls
- Literal `@`, `#`, `$`, `*`, `_`, `<` in prose must be escaped with `\` (e.g. email addresses, hashtags, prices).
- A stray `*` or `_` silently toggles bold/emphasis for the rest of the paragraph.
- Do not mix manual numbering with automatic numbering.

## Working in LetsWritePaper
- Read the relevant `.typ` and `.bib` files with `read_document` before editing.
- Prefer small targeted `edit_document {path, old_text, new_text, rationale}` calls whose `old_text` is an exact, unique excerpt.
- Never invent bibliography entries or keys; if a reference is missing, insert `TODO: cite ...` and tell the user.
