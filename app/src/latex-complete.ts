// Overleaf-style LaTeX completion: typing "\" suggests commands, accepting one
// fills its arguments, and Tab moves through those fields. Citation keys,
// labels, packages, and files come from the open paper.
import {
  acceptCompletion,
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  insertCompletionText,
  pickedCompletion,
  snippet,
  snippetCompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSection,
} from "@codemirror/autocomplete";
import { indentUnit } from "@codemirror/language";
import { Prec, type Extension } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";

export interface LatexHost {
  listDocs: () => readonly string[];
  readDoc: (path: string) => Promise<string>;
}

type LatexCompletion = Completion & { reopen?: boolean };

const SEC_CMD: CompletionSection = { name: "Commands", rank: 0 };
const SEC_SYM: CompletionSection = { name: "Symbols", rank: 1 };
const SEC_ENV: CompletionSection = { name: "Environments", rank: 1 };
const SEC_OPEN: CompletionSection = { name: "Open", rank: 0 };
const SEC_CITE: CompletionSection = { name: "Citation keys", rank: 0 };
const SEC_LABEL: CompletionSection = { name: "Labels", rank: 0 };
const SEC_PKG: CompletionSection = { name: "Packages", rank: 0 };
const SEC_FILE: CompletionSection = { name: "Files", rank: 0 };
const SEC_ENTRY: CompletionSection = { name: "Entry types", rank: 0 };
const SEC_FIELD: CompletionSection = { name: "Fields", rank: 0 };

const F = (n: number) => "${" + n + "}";
const F0 = "${0}";

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function inComment(before: string): boolean {
  let escaped = false;
  for (const ch of before) {
    if (escaped) {
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      continue;
    }
    if (ch === "%") return true;
  }
  return false;
}

function braced(n: number): string {
  return "{" + F(n) + "}";
}

function command(name: string, detail: string, args?: string, boost = 20, reopen = false): Completion {
  const base: LatexCompletion = {
    label: "\\" + name,
    detail,
    type: "function",
    boost,
    section: SEC_CMD,
  };
  if (args == null) return base;
  const made = snippetCompletion("\\" + name + args, base);
  if (reopen) (made as LatexCompletion).reopen = true;
  return made;
}

function symbol(name: string, detail: string, boost = 0): Completion {
  return { label: "\\" + name, detail, type: "constant", boost, section: SEC_SYM };
}

interface EnvSpec {
  name: string;
  /** Text between `\begin{name}` and `\end{name}`. Leading tabs are indented. */
  middle?: string;
  /** Extra arguments right after the environment name, e.g. `{cc}`. */
  extra?: string;
  boost?: number;
  /** Also list `\begin{name}` while a command is being typed. */
  menu?: boolean;
}

function envMiddle(spec: EnvSpec): string {
  return spec.middle ?? "\n\t" + F0 + "\n";
}

/** Replacement used when the cursor is already inside `\begin{`. */
function envInside(spec: EnvSpec): string {
  return spec.name + "}" + (spec.extra ?? "") + envMiddle(spec) + "\\end{" + spec.name + "}";
}

function envFull(spec: EnvSpec): string {
  return "\\begin{" + envInside(spec);
}

const figureMiddle =
  "\n\t\\centering\n\t\\includegraphics[width=\\linewidth]{" +
  F(1) +
  "}\n\t\\caption{" +
  F(2) +
  "}\n\t\\label{fig:" +
  F(3) +
  "}\n";

const tableMiddle =
  "\n\t\\centering\n\t\\caption{" +
  F(1) +
  "}\n\t\\label{tab:" +
  F(2) +
  "}\n\t\\begin{tabular}{" +
  F(3) +
  "}\n\t\t" +
  F0 +
  "\n\t\\end{tabular}\n";

const eqMiddle = "\n\t" + F(1) + "\n\t\\label{eq:" + F(2) + "}\n";
const listMiddle = "\n\t\\item " + F0 + "\n";
const descMiddle = "\n\t\\item[" + F(1) + "] " + F0 + "\n";
const cols = "{" + F(1) + "}";

const ENV_SPECS: EnvSpec[] = [
  { name: "document", boost: 30 },
  { name: "abstract", boost: 24, menu: true },
  { name: "figure", boost: 50, menu: true, middle: figureMiddle },
  { name: "figure*", boost: 22, menu: true, middle: figureMiddle },
  { name: "table", boost: 48, menu: true, middle: tableMiddle },
  { name: "table*", boost: 20, menu: true, middle: tableMiddle },
  { name: "tabular", boost: 16, extra: cols },
  { name: "tabularx", boost: 8, extra: cols },
  { name: "array", boost: 8, extra: cols },
  { name: "equation", boost: 46, menu: true, middle: eqMiddle },
  { name: "equation*", boost: 28, menu: true },
  { name: "align", boost: 44, menu: true, middle: eqMiddle },
  { name: "align*", boost: 26, menu: true },
  { name: "alignat", boost: 6, extra: cols, middle: eqMiddle },
  { name: "gather", boost: 12, menu: true, middle: eqMiddle },
  { name: "gather*", boost: 6 },
  { name: "multline", boost: 6, middle: eqMiddle },
  { name: "multline*", boost: 4 },
  { name: "flalign", boost: 4, middle: eqMiddle },
  { name: "split", boost: 10 },
  { name: "aligned", boost: 10 },
  { name: "gathered", boost: 4 },
  { name: "cases", boost: 14, menu: true },
  { name: "subequations", boost: 8 },
  { name: "matrix", boost: 8 },
  { name: "pmatrix", boost: 12 },
  { name: "bmatrix", boost: 12 },
  { name: "Bmatrix", boost: 4 },
  { name: "vmatrix", boost: 4 },
  { name: "Vmatrix", boost: 4 },
  { name: "smallmatrix", boost: 4 },
  { name: "itemize", boost: 36, menu: true, middle: listMiddle },
  { name: "enumerate", boost: 34, menu: true, middle: listMiddle },
  { name: "description", boost: 10, middle: descMiddle },
  { name: "quote", boost: 4 },
  { name: "quotation", boost: 4 },
  { name: "verse", boost: 2 },
  { name: "center", boost: 6 },
  { name: "flushleft", boost: 2 },
  { name: "flushright", boost: 2 },
  { name: "minipage", boost: 4 },
  { name: "verbatim", boost: 6, menu: true },
  { name: "lstlisting", boost: 4 },
  { name: "minted", boost: 4, extra: cols },
  { name: "tikzpicture", boost: 8 },
  { name: "tikzcd", boost: 4 },
  { name: "algorithm", boost: 6 },
  { name: "algorithmic", boost: 6 },
  { name: "proof", boost: 18, menu: true },
  { name: "theorem", boost: 16 },
  { name: "lemma", boost: 14 },
  { name: "proposition", boost: 10 },
  { name: "corollary", boost: 10 },
  { name: "definition", boost: 12 },
  { name: "remark", boost: 6 },
  { name: "example", boost: 6 },
  { name: "assumption", boost: 4 },
  { name: "claim", boost: 4 },
  { name: "thebibliography", boost: 4, extra: cols, middle: "\n\t\\bibitem{" + F(2) + "} " + F0 + "\n" },
  { name: "frame", boost: 4 },
  { name: "columns", boost: 2 },
  { name: "column", boost: 2 },
  { name: "block", boost: 2 },
  { name: "alertblock", boost: 2 },
  { name: "multicols", boost: 2, extra: cols },
  { name: "refsection", boost: 2 },
];

const ENV_BY_NAME = new Map(ENV_SPECS.map((spec) => [spec.name, spec]));

function dispatchInsert(view: EditorView, completion: Completion, from: number, to: number, text: string) {
  view.dispatch({
    ...insertCompletionText(view.state, text, from, to),
    annotations: pickedCompletion.of(completion),
  });
}

function applyEnvironment(view: EditorView, completion: Completion, from: number, to: number) {
  const spec = ENV_BY_NAME.get(completion.label) ?? { name: completion.label };
  const { state } = view;
  // `\begin{${}}…\end{${}}` selects both names. A plain name fills both; a
  // skeleton (figure, equation, …) replaces that whole block.
  if (state.selection.ranges.length > 1) {
    const rich = spec.middle != null || spec.extra != null;
    const beginAt = state.sliceDoc(0, from).lastIndexOf("\\begin{");
    const end = rich ? /\\end\{[^}]*\}/.exec(state.sliceDoc(to, Math.min(state.doc.length, to + 500))) : null;
    if (rich && beginAt >= 0 && end) {
      snippet(envFull(spec))(view, completion, beginAt, to + end.index + end[0].length);
      return;
    }
    dispatchInsert(view, completion, from, to, spec.name);
    return;
  }
  const after = state.sliceDoc(to, Math.min(state.doc.length, to + 600));
  const eat = after.startsWith("}") ? 1 : 0;
  const rest = after.slice(eat);
  if (new RegExp("\\\\end\\s*\\{\\s*" + escapeRegExp(spec.name) + "\\s*\\}").test(rest)) {
    dispatchInsert(view, completion, from, to, spec.name);
    return;
  }
  snippet(envInside(spec))(view, completion, from, to + eat);
}

function applyEnd(view: EditorView, completion: Completion, from: number, to: number) {
  const after = view.state.sliceDoc(to, to + 1);
  dispatchInsert(view, completion, from, to, after === "}" ? completion.label : completion.label + "}");
}

function applyInside(view: EditorView, completion: Completion, from: number, to: number) {
  const after = view.state.sliceDoc(to, to + 1);
  const text = after === "}" ? completion.label : completion.label + "}";
  const spec = insertCompletionText(view.state, text, from, to);
  view.dispatch({
    ...spec,
    selection: { anchor: from + completion.label.length },
    annotations: pickedCompletion.of(completion),
  });
}

function unclosedEnvs(text: string): string[] {
  const stack: string[] = [];
  const re = /\\(begin|end)\{([^{}]+)\}/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text))) {
    const name = match[2].trim();
    if (match[1] === "begin") stack.push(name);
    else {
      const at = stack.lastIndexOf(name);
      if (at >= 0) stack.splice(at, 1);
    }
  }
  return stack;
}

function environmentOptions(open: readonly string[], ending: boolean): Completion[] {
  const rank = new Map<string, number>();
  open.forEach((name, i) => rank.set(name, open.length - i));
  const names = new Set<string>([...ENV_SPECS.map((s) => s.name), ...open]);
  return [...names].map((name) => ({
    label: name,
    detail: rank.has(name) ? "open" : "environment",
    type: "type",
    boost: (rank.get(name) ?? 0) * 20 + (ENV_BY_NAME.get(name)?.boost ?? 0),
    section: rank.has(name) ? SEC_OPEN : SEC_ENV,
    apply: ending ? applyEnd : applyEnvironment,
  }));
}

const POPULAR = 50;

const COMMANDS: readonly Completion[] = [
  command("begin", "environment", "{" + F(1) + "}\n\t" + F0 + "\n\\end{" + F(1) + "}", POPULAR, true),
  command("end", "environment", braced(1), 24, true),
  ...ENV_SPECS.filter((spec) => spec.menu).map((spec) =>
    snippetCompletion(envFull(spec), {
      label: "\\begin{" + spec.name + "}",
      detail: spec.name,
      type: "type",
      boost: 12,
      section: SEC_CMD,
    }),
  ),
  command("section", "section", braced(1), POPULAR),
  command("section*", "unnumbered section", braced(1), 18),
  command("subsection", "subsection", braced(1), 40),
  command("subsection*", "unnumbered subsection", braced(1), 12),
  command("subsubsection", "subsubsection", braced(1), 28),
  command("subsubsection*", "unnumbered subsubsection", braced(1), 8),
  command("paragraph", "paragraph", braced(1), 10),
  command("subparagraph", "subparagraph", braced(1), 6),
  command("chapter", "chapter", braced(1), 16),
  command("chapter*", "unnumbered chapter", braced(1), 6),
  command("part", "part", braced(1), 6),
  command("title", "title", braced(1), 16),
  command("author", "author", braced(1), 14),
  command("date", "date", braced(1), 8),
  command("thanks", "thanks", braced(1), 4),
  command("maketitle", "title block", undefined, 8),
  command("tableofcontents", "contents", undefined, 4),
  command("appendix", "appendix", undefined, 6),
  command("documentclass", "class", braced(1), 16, true),
  command("usepackage", "package", braced(1), 36, true),
  command("RequirePackage", "package", braced(1), 4, true),
  command("input", "file", braced(1), 22, true),
  command("include", "file", braced(1), 14, true),
  command("subfile", "file", braced(1), 4, true),
  command("includegraphics", "image", "[width=\\linewidth]{" + F(1) + "}", 32, true),
  command("bibliography", "bibliography", braced(1), 12, true),
  command("addbibresource", "bibliography", braced(1), 10, true),
  command("printbibliography", "print bibliography", undefined, 8),
  command("cite", "citation", braced(1), POPULAR, true),
  command("citep", "parenthetical citation", braced(1), 30, true),
  command("citet", "textual citation", braced(1), 28, true),
  command("citealp", "citation", braced(1), 6, true),
  command("citealt", "citation", braced(1), 6, true),
  command("citeauthor", "author", braced(1), 8, true),
  command("citeyear", "year", braced(1), 6, true),
  command("parencite", "citation", braced(1), 16, true),
  command("textcite", "citation", braced(1), 16, true),
  command("autocite", "citation", braced(1), 8, true),
  command("footcite", "citation", braced(1), 4, true),
  command("nocite", "citation", braced(1), 4, true),
  command("ref", "reference", braced(1), POPULAR, true),
  command("eqref", "equation reference", braced(1), 34, true),
  command("pageref", "page reference", braced(1), 8, true),
  command("autoref", "reference", braced(1), 14, true),
  command("cref", "reference", braced(1), 18, true),
  command("Cref", "reference", braced(1), 12, true),
  command("nameref", "reference", braced(1), 4, true),
  command("vref", "reference", braced(1), 4, true),
  command("label", "label", braced(1), 42, true),
  command("caption", "caption", braced(1), 24),
  command("item", "list item", " " + F0, 36),
  command("bibitem", "bibliography item", braced(1) + " " + F0, 6),
  command("footnote", "footnote", braced(1), 16),
  command("emph", "emphasis", braced(1), 22),
  command("textbf", "bold", braced(1), 26),
  command("textit", "italic", braced(1), 20),
  command("texttt", "typewriter", braced(1), 12),
  command("textrm", "roman", braced(1), 8),
  command("textsf", "sans serif", braced(1), 4),
  command("textsc", "small caps", braced(1), 4),
  command("underline", "underline", braced(1), 6),
  command("text", "text in math", braced(1), 14),
  command("frac", "fraction", braced(1) + braced(2), POPULAR),
  command("dfrac", "display fraction", braced(1) + braced(2), 10),
  command("tfrac", "text fraction", braced(1) + braced(2), 6),
  command("sqrt", "square root", braced(1), 30),
  snippetCompletion("\\sqrt[" + F(1) + "]{" + F(2) + "}", {
    label: "\\sqrt[]",
    detail: "nth root",
    type: "function",
    boost: 8,
    section: SEC_CMD,
  }),
  command("binom", "binomial", braced(1) + braced(2), 8),
  command("sum", "sum", undefined, 18),
  command("int", "integral", undefined, 16),
  command("prod", "product", undefined, 8),
  command("lim", "limit", undefined, 10),
  command("infty", "infinity", undefined, 8),
  command("partial", "partial", undefined, 6),
  command("mathbb", "blackboard bold", braced(1), 12),
  command("mathbf", "bold math", braced(1), 12),
  command("mathcal", "calligraphic", braced(1), 10),
  command("mathrm", "roman math", braced(1), 10),
  command("mathit", "italic math", braced(1), 4),
  command("mathsf", "sans math", braced(1), 4),
  command("mathtt", "mono math", braced(1), 4),
  command("mathfrak", "fraktur", braced(1), 4),
  command("operatorname", "operator", braced(1), 10),
  command("hat", "hat", braced(1), 6),
  command("bar", "bar", braced(1), 6),
  command("vec", "vector", braced(1), 6),
  command("tilde", "tilde", braced(1), 6),
  command("dot", "dot", braced(1), 4),
  command("ddot", "double dot", braced(1), 4),
  command("overline", "overline", braced(1), 6),
  command("widehat", "wide hat", braced(1), 4),
  command("widetilde", "wide tilde", braced(1), 4),
  command("overrightarrow", "arrow", braced(1), 4),
  snippetCompletion("\\left(" + F(1) + "\\right)", {
    label: "\\left(",
    detail: "parentheses",
    type: "function",
    boost: 14,
    section: SEC_CMD,
  }),
  snippetCompletion("\\left[" + F(1) + "\\right]", {
    label: "\\left[",
    detail: "brackets",
    type: "function",
    boost: 8,
    section: SEC_CMD,
  }),
  snippetCompletion("\\left\\\\{" + F(1) + "\\right\\\\}", {
    label: "\\left\\{",
    detail: "braces",
    type: "function",
    boost: 8,
    section: SEC_CMD,
  }),
  snippetCompletion("\\left|" + F(1) + "\\right|", {
    label: "\\left|",
    detail: "bars",
    type: "function",
    boost: 6,
    section: SEC_CMD,
  }),
  command("href", "link", braced(1) + braced(2), 8),
  command("url", "url", braced(1), 6),
  command("newline", "line break", undefined, 4),
  command("noindent", "no indent", undefined, 4),
  command("centering", "center", undefined, 8),
  command("linewidth", "line width", undefined, 4),
  command("textwidth", "text width", undefined, 4),
  command("newcommand", "new command", "{\\" + F(1) + "}" + braced(2), 8),
  command("renewcommand", "renew command", "{\\" + F(1) + "}" + braced(2), 6),
  command("newenvironment", "new environment", braced(1) + braced(2) + braced(3), 4),
  command("newtheorem", "theorem", braced(1) + braced(2), 6),
  ...[
    ["alpha", "α"],
    ["beta", "β"],
    ["gamma", "γ"],
    ["delta", "δ"],
    ["epsilon", "ϵ"],
    ["varepsilon", "ε"],
    ["zeta", "ζ"],
    ["eta", "η"],
    ["theta", "θ"],
    ["vartheta", "ϑ"],
    ["iota", "ι"],
    ["kappa", "κ"],
    ["lambda", "λ"],
    ["mu", "μ"],
    ["nu", "ν"],
    ["xi", "ξ"],
    ["pi", "π"],
    ["varpi", "ϖ"],
    ["rho", "ρ"],
    ["varrho", "ϱ"],
    ["sigma", "σ"],
    ["varsigma", "ς"],
    ["tau", "τ"],
    ["upsilon", "υ"],
    ["phi", "φ"],
    ["varphi", "ϕ"],
    ["chi", "χ"],
    ["psi", "ψ"],
    ["omega", "ω"],
    ["Gamma", "Γ"],
    ["Delta", "Δ"],
    ["Theta", "Θ"],
    ["Lambda", "Λ"],
    ["Xi", "Ξ"],
    ["Pi", "Π"],
    ["Sigma", "Σ"],
    ["Upsilon", "Υ"],
    ["Phi", "Φ"],
    ["Psi", "Ψ"],
    ["Omega", "Ω"],
    ["leq", "≤"],
    ["geq", "≥"],
    ["neq", "≠"],
    ["approx", "≈"],
    ["equiv", "≡"],
    ["sim", "∼"],
    ["simeq", "≃"],
    ["propto", "∝"],
    ["ll", "≪"],
    ["gg", "≫"],
    ["pm", "±"],
    ["mp", "∓"],
    ["times", "×"],
    ["cdot", "·"],
    ["div", "÷"],
    ["oplus", "⊕"],
    ["otimes", "⊗"],
    ["circ", "∘"],
    ["bullet", "•"],
    ["star", "⋆"],
    ["to", "→"],
    ["rightarrow", "→"],
    ["leftarrow", "←"],
    ["Rightarrow", "⇒"],
    ["Leftarrow", "⇐"],
    ["leftrightarrow", "↔"],
    ["Leftrightarrow", "⇔"],
    ["mapsto", "↦"],
    ["implies", "⟹"],
    ["iff", "⟺"],
    ["in", "∈"],
    ["notin", "∉"],
    ["subset", "⊂"],
    ["subseteq", "⊆"],
    ["supset", "⊃"],
    ["supseteq", "⊇"],
    ["cup", "∪"],
    ["cap", "∩"],
    ["setminus", "∖"],
    ["emptyset", "∅"],
    ["forall", "∀"],
    ["exists", "∃"],
    ["neg", "¬"],
    ["land", "∧"],
    ["lor", "∨"],
    ["nabla", "∇"],
    ["angle", "∠"],
    ["langle", "⟨"],
    ["rangle", "⟩"],
    ["lfloor", "⌊"],
    ["rfloor", "⌋"],
    ["lceil", "⌈"],
    ["rceil", "⌉"],
    ["vert", "|"],
    ["Vert", "‖"],
    ["ldots", "…"],
    ["cdots", "⋯"],
    ["vdots", "⋮"],
    ["ddots", "⋱"],
    ["quad", "quad"],
    ["qquad", "qquad"],
    ["sin", "sin"],
    ["cos", "cos"],
    ["tan", "tan"],
    ["log", "log"],
    ["ln", "ln"],
    ["exp", "exp"],
    ["min", "min"],
    ["max", "max"],
    ["sup", "sup"],
    ["inf", "inf"],
    ["det", "det"],
    ["dim", "dim"],
    ["ker", "ker"],
    ["arg", "arg"],
    ["gcd", "gcd"],
    ["deg", "deg"],
  ].map(([name, detail]) => symbol(name, detail, name.length <= 5 ? 4 : 0)),
];

const PACKAGES = [
  "amsmath",
  "amssymb",
  "amsthm",
  "amsfonts",
  "mathtools",
  "graphicx",
  "booktabs",
  "hyperref",
  "geometry",
  "xcolor",
  "cleveref",
  "natbib",
  "biblatex",
  "microtype",
  "inputenc",
  "fontenc",
  "babel",
  "csquotes",
  "setspace",
  "enumitem",
  "caption",
  "subcaption",
  "multirow",
  "array",
  "tabularx",
  "longtable",
  "siunitx",
  "algorithm2e",
  "algpseudocode",
  "listings",
  "minted",
  "tikz",
  "pgfplots",
  "float",
  "placeins",
  "wrapfig",
  "url",
  "xurl",
  "doi",
  "authblk",
  "lineno",
  "todonotes",
  "soul",
  "ulem",
  "cancel",
  "bm",
  "braket",
  "nicefrac",
  "mhchem",
  "tcolorbox",
  "mdframed",
  "fancyhdr",
  "titlesec",
  "appendix",
  "pdflscape",
  "adjustbox",
  "subfig",
  "epstopdf",
  "lmodern",
  "newtxtext",
  "newtxmath",
  "mathpazo",
  "etoolbox",
  "xparse",
  "comment",
  "verbatim",
  "fancyvrb",
  "stmaryrd",
  "dsfont",
  "mathrsfs",
  "bbm",
  "makecell",
  "colortbl",
  "threeparttable",
  "ragged2e",
  "multicol",
  "footmisc",
  "acronym",
  "glossaries",
  "imakeidx",
  "tikz-cd",
].map(
  (name): Completion => ({
    label: name,
    detail: "package",
    type: "keyword",
    section: SEC_PKG,
    apply: applyInside,
  }),
);

const CLASSES = [
  "article",
  "report",
  "book",
  "memoir",
  "beamer",
  "acmart",
  "IEEEtran",
  "llncs",
  "elsarticle",
  "revtex4-2",
  "scrartcl",
  "scrreprt",
  "scrbook",
  "standalone",
  "proc",
  "letter",
].map(
  (name): Completion => ({
    label: name,
    detail: "class",
    type: "class",
    section: SEC_PKG,
    apply: applyInside,
  }),
);

const CITE = new Set([
  "cite",
  "citet",
  "citep",
  "citealp",
  "citealt",
  "citeauthor",
  "citeyear",
  "citeyearpar",
  "Cite",
  "Citet",
  "Citep",
  "footcite",
  "parencite",
  "textcite",
  "autocite",
  "smartcite",
  "supercite",
  "cites",
  "nocite",
  "fullcite",
]);
const REF = new Set(["ref", "eqref", "pageref", "autoref", "nameref", "cref", "Cref", "vref", "vpageref", "label", "subref"]);

type ArgKind = "env" | "end" | "cite" | "ref" | "pkg" | "class" | "tex" | "bib" | "bibext" | "image";

function argumentKind(commandName: string): ArgKind | null {
  const name = commandName.replace(/\*$/, "");
  if (name === "begin") return "env";
  if (name === "end") return "end";
  if (CITE.has(name)) return "cite";
  if (REF.has(name)) return "ref";
  if (name === "usepackage" || name === "RequirePackage") return "pkg";
  if (name === "documentclass" || name === "LoadClass") return "class";
  if (name === "input" || name === "include" || name === "subfile" || name === "InputIfFileExists") return "tex";
  if (name === "bibliography") return "bib";
  if (name === "addbibresource") return "bibext";
  if (name === "includegraphics") return "image";
  return null;
}

function partialRange(pos: number, raw: string): { from: number; partial: string; used: Set<string> } {
  const comma = raw.lastIndexOf(",");
  const after = comma >= 0 ? raw.slice(comma + 1) : raw;
  const partial = after.trimStart();
  const used = new Set(
    (comma >= 0 ? raw.slice(0, comma) : "")
      .split(",")
      .map((part) => part.trim())
      .filter(Boolean),
  );
  return { from: pos - partial.length, partial, used };
}

function withoutUsed(options: readonly Completion[], used: Set<string>): Completion[] {
  if (!used.size) return [...options];
  return options.filter((option) => !used.has(option.label));
}

function fileOptions(host: LatexHost, kind: "tex" | "bib" | "bibext" | "image"): Completion[] {
  const out: Completion[] = [];
  const seen = new Set<string>();
  for (const path of host.listDocs()) {
    let label: string | null = null;
    if (kind === "tex" && path.endsWith(".tex")) label = path.replace(/\.tex$/, "");
    else if ((kind === "bib" || kind === "bibext") && path.endsWith(".bib")) {
      label = kind === "bibext" ? path : path.replace(/\.bib$/, "");
    } else if (kind === "image" && /\.(png|jpe?g|pdf|eps|svg)$/i.test(path)) label = path;
    if (!label || seen.has(label)) continue;
    seen.add(label);
    out.push({ label, detail: path, type: "text", section: SEC_FILE, apply: applyInside });
  }
  return out;
}

const CITE_RE = /@(?!string\b|comment\b|preamble\b)[A-Za-z]+\s*\{\s*([^,\s]+)/g;
const LABEL_RE = /\\label\s*\{([^}]+)\}/g;
const BIBITEM_RE = /\\bibitem\s*(?:\[[^\]]*\])?\s*\{([^}]+)\}/g;

function scanCites(text: string, into: Set<string>) {
  for (const match of text.matchAll(CITE_RE)) into.add(match[1]);
  for (const match of text.matchAll(BIBITEM_RE)) into.add(match[1]);
}

function scanLabels(text: string, into: Set<string>) {
  for (const match of text.matchAll(LABEL_RE)) into.add(match[1]);
}

interface SymbolCache {
  key: string;
  at: number;
  cites: string[];
  labels: string[];
}

let symbolCache: SymbolCache | null = null;

async function otherSymbols(host: LatexHost, selfPath: string): Promise<{ cites: string[]; labels: string[] }> {
  const files = host.listDocs();
  const key = selfPath + "\0" + files.join("\0");
  const now = Date.now();
  if (symbolCache && symbolCache.key === key && now - symbolCache.at < 1500) return symbolCache;
  const cites = new Set<string>();
  const labels = new Set<string>();
  await Promise.all(
    files.map(async (path) => {
      if (path === selfPath) return;
      let text = "";
      try {
        text = await host.readDoc(path);
      } catch {
        return;
      }
      if (path.endsWith(".bib")) scanCites(text, cites);
      if (/\.(tex|sty|cls)$/.test(path)) {
        scanLabels(text, labels);
        scanCites(text, cites);
      }
    }),
  );
  symbolCache = { key, at: now, cites: [...cites], labels: [...labels] };
  return symbolCache;
}

function keyCompletions(keys: readonly string[], section: CompletionSection, detail: string): Completion[] {
  return keys.map((label) => ({ label, detail, type: "text", section, apply: applyInside }));
}

const ARG_RE = /\\([a-zA-Z@]+)(\*?)(?:\s*\[[^\]]*\])*\s*\{([^}]*)$/;

function completeArgument(
  context: CompletionContext,
  path: string,
  host: LatexHost,
  commandName: string,
  raw: string,
): CompletionResult | null | Promise<CompletionResult | null> {
  const kind = argumentKind(commandName);
  if (!kind) return null;
  const { state, pos } = context;
  const { from, used } = partialRange(pos, raw);
  const listed = (options: readonly Completion[], validFor: RegExp): CompletionResult | null => {
    const filtered = withoutUsed(options, used);
    return filtered.length ? { from, options: filtered, validFor } : null;
  };

  if (kind === "env" || kind === "end") {
    return listed(environmentOptions(unclosedEnvs(state.sliceDoc(0, from)), kind === "end"), /^[A-Za-z0-9*]*$/);
  }
  if (kind === "pkg") return listed(PACKAGES, /^[A-Za-z0-9@-]*$/);
  if (kind === "class") return listed(CLASSES, /^[A-Za-z0-9-]*$/);
  if (kind === "tex" || kind === "bib" || kind === "bibext" || kind === "image") {
    return listed(fileOptions(host, kind), /^[^},]*$/);
  }

  const live = state.doc.toString();
  return otherSymbols(host, path).then((cached) => {
    if (context.aborted) return null;
    if (kind === "cite") {
      const cites = new Set(cached.cites);
      scanCites(live, cites);
      return listed(keyCompletions([...cites], SEC_CITE, "citation"), /^[^},]*$/);
    }
    const labels = new Set(cached.labels);
    scanLabels(live, labels);
    return listed(keyCompletions([...labels], SEC_LABEL, "label"), /^[^},]*$/);
  });
}

/** Completion source for `.tex`, `.sty`, and `.cls` documents. */
export function completeLatex(
  context: CompletionContext,
  path: string,
  host: LatexHost,
): CompletionResult | null | Promise<CompletionResult | null> {
  const { state, pos } = context;
  const line = state.doc.lineAt(pos);
  const before = line.text.slice(0, pos - line.from);
  if (inComment(before)) return null;

  const arg = ARG_RE.exec(before);
  if (arg) return completeArgument(context, path, host, arg[1] + arg[2], arg[3]);

  const typed = context.matchBefore(/\\[a-zA-Z@*]*$/);
  // A lone "\\" is a line break. "\\\cmd" still completes the command.
  const lineBreak = typed?.text === "\\" && state.sliceDoc(typed.from - 1, typed.from) === "\\";
  if (typed && typed.text.startsWith("\\") && !lineBreak) {
    return { from: typed.from, options: COMMANDS, validFor: /^\\[a-zA-Z@*]*$/ };
  }
  if (context.explicit) return { from: pos, options: COMMANDS, validFor: /^\\?[a-zA-Z@*]*$/ };
  return null;
}

/**
 * When `}` closes a `\begin{name}`, insert a matching `\end{name}` and leave
 * the cursor in the body. Returns null when this `}` is not that closer.
 */
export function environmentClose(
  before: string,
  after: string,
  indent: string,
): { insert: string; eatNextBrace: boolean; cursor: number } | null {
  if (inComment(before)) return null;
  const found = before.match(/\\begin\{([A-Za-z*][A-Za-z0-9*]*)$/);
  if (!found) return null;
  const name = found[1];
  if (new RegExp("\\\\end\\s*\\{\\s*" + escapeRegExp(name) + "\\s*\\}").test(after)) return null;
  const insert = "}\n" + indent + "\n\\end{" + name + "}";
  return { insert, eatNextBrace: after.startsWith("}"), cursor: 2 + indent.length };
}

function closeBeginEnvironment(view: EditorView, from: number, to: number, text: string): boolean {
  if (text !== "}" || from !== to) return false;
  const start = Math.max(0, from - 240);
  const change = environmentClose(
    view.state.sliceDoc(start, from),
    view.state.sliceDoc(to, Math.min(view.state.doc.length, to + 600)),
    view.state.facet(indentUnit),
  );
  if (!change) return false;
  view.dispatch({
    changes: { from, to: to + (change.eatNextBrace ? 1 : 0), insert: change.insert },
    selection: { anchor: from + change.cursor },
    userEvent: "input.type",
  });
  return true;
}

const BIB_TYPES: { type: string; fields: string[] }[] = [
  { type: "article", fields: ["author", "title", "journal", "year", "volume", "number", "pages", "doi"] },
  { type: "inproceedings", fields: ["author", "title", "booktitle", "year", "pages", "doi"] },
  { type: "book", fields: ["author", "title", "publisher", "year"] },
  { type: "incollection", fields: ["author", "title", "booktitle", "publisher", "year", "pages"] },
  { type: "phdthesis", fields: ["author", "title", "school", "year"] },
  { type: "mastersthesis", fields: ["author", "title", "school", "year"] },
  { type: "techreport", fields: ["author", "title", "institution", "year"] },
  { type: "misc", fields: ["author", "title", "howpublished", "year", "note"] },
  { type: "unpublished", fields: ["author", "title", "note", "year"] },
  { type: "online", fields: ["author", "title", "year", "url"] },
];

const BIB_FIELDS = [
  "author",
  "title",
  "journal",
  "booktitle",
  "year",
  "volume",
  "number",
  "pages",
  "publisher",
  "school",
  "institution",
  "editor",
  "doi",
  "url",
  "note",
  "series",
  "address",
  "month",
  "organization",
  "howpublished",
  "abstract",
  "isbn",
  "eprint",
  "keywords",
];

function bibEntry(type: string, fields: string[]): Completion {
  const lines = fields.map((field, i) => "\t" + field + " = {" + F(i + 2) + "},");
  return snippetCompletion("@" + type + "{${1:key},\n" + lines.join("\n") + "\n}", {
    label: "@" + type,
    detail: type,
    type: "type",
    section: SEC_ENTRY,
  });
}

const BIB_ENTRIES = BIB_TYPES.map((entry) => bibEntry(entry.type, entry.fields));
const BIB_FIELD_COMPLETIONS: Completion[] = BIB_FIELDS.map((field) =>
  snippetCompletion(field + " = {" + F(1) + "},", {
    label: field,
    detail: "field",
    type: "property",
    section: SEC_FIELD,
  }),
);

/** Completion source for `.bib` files. */
export function completeBib(context: CompletionContext): CompletionResult | null {
  const { state, pos } = context;
  const line = state.doc.lineAt(pos);
  const before = line.text.slice(0, pos - line.from);
  const entry = before.match(/@[A-Za-z]*$/);
  if (entry && entry[0].startsWith("@")) {
    return { from: pos - entry[0].length, options: BIB_ENTRIES, validFor: /^@[A-Za-z]*$/ };
  }
  if (/^\s*[A-Za-z]*$/.test(before) && (before.trim().length > 0 || context.explicit)) {
    const word = before.trimStart();
    return { from: pos - word.length, options: BIB_FIELD_COMPLETIONS, validFor: /^[A-Za-z]*$/ };
  }
  return null;
}

function completionConfig(source: (context: CompletionContext) => CompletionResult | null | Promise<CompletionResult | null>): Extension {
  return autocompletion({
    override: [source],
    activateOnCompletion: (completion) => Boolean((completion as LatexCompletion).reopen),
    activateOnTypingDelay: 0,
    filterStrict: true,
    interactionDelay: 0,
    maxRenderedOptions: 80,
  });
}

/** Suggestions, argument snippets, bracket pairs, and Tab-to-accept. */
export function latexSupport(path: string, host: LatexHost): Extension {
  return [
    completionConfig((context) => completeLatex(context, path, host)),
    closeBrackets(),
    Prec.high(keymap.of(closeBracketsKeymap)),
    Prec.high(EditorView.inputHandler.of(closeBeginEnvironment)),
    Prec.highest(keymap.of([{ key: "Tab", run: acceptCompletion }])),
  ];
}

/** Suggestions for bibliography databases. */
export function bibSupport(): Extension {
  return [
    completionConfig(completeBib),
    closeBrackets(),
    Prec.high(keymap.of(closeBracketsKeymap)),
    Prec.highest(keymap.of([{ key: "Tab", run: acceptCompletion }])),
  ];
}
