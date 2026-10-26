---
name: clarity-editing
description: Line-level editing pass for academic prose — make statements concrete, cut hedging and filler, remove AI-sounding phrasing, unify terminology, and keep one idea per paragraph, while preserving meaning, citations, labels, and math exactly. Use when polishing any section.
kind: style
sections: []
---

# Clarity Editing

This is a line edit, not a rewrite. The author's claims, structure, and voice stay; the words get tighter and more precise.

## Hard constraints (never violate)
- Preserve meaning. Do not strengthen, weaken, or add claims. If a sentence is ambiguous, ask or leave a comment rather than guess.
- Preserve every citation command/key, `\label`/`<label>`, cross-reference, footnote, and math expression exactly,
  character for character. Never retype an equation.
- Do not change numbers, units, dataset names, or method names.
- Do not change the field's established terminology to synonyms.

## Concrete over vague
- Replace vague quantifiers with the actual value: "significantly faster" → "3.2× faster"; "a large dataset" → "1.2M sentences".
- Replace abstract nouns with the actor and action: "The utilization of X enables an improvement of Y" → "X improves Y".
- Name the thing instead of pointing: "this" alone at sentence start → "this constraint", "this result".

## Cut hedging and filler
- One hedge per claim at most, and only where uncertainty is real: "may potentially suggest" → "suggests".
- Delete throat-clearing: "It is important to note that", "In order to" (→ "to"), "the fact that", "basically", "very", "quite".
- Delete sentences that announce instead of say: "In this section, we will discuss the results." → start with the result.

## Banned AI-slop patterns
Remove or rewrite these on sight:
- Words: "delve", "delve into", "tapestry", "intricate", "multifaceted", "pivotal", "paramount", "seamless(ly)",
  "leverage" (when "use" works), "utilize", "showcase", "underscore(s)", "robust" (unless technically defined), "holistic",
  "cutting-edge", "groundbreaking", "game-changer", "unlock", "harness the power", "navigate the complexities".
- Phrases: "it is worth noting that", "it should be noted", "plays a crucial/pivotal/vital role in", "in the realm of",
  "in today's fast-paced world", "in the ever-evolving landscape of", "a testament to", "paves the way for",
  "sheds light on", "a deep dive", "at the intersection of", "not only ... but also" used for emphasis.
- Structures: empty triads ("efficient, scalable, and robust" with no evidence for each); "X is not just A, it's B";
  rhetorical questions as transitions; sentences ending in "..., highlighting/underscoring the importance of Z";
  closing every paragraph with a summary sentence that repeats its first sentence.
- Typography: em-dash chains used as a stylistic tic; title-case headings when the template uses sentence case; emoji.

## Consistent terminology
- One name per concept across the paper. If the method is "the encoder" in Section 3, it is not "the embedding module" in Section 5.
- Define acronyms once, at first use in the body (separately from the abstract), then use only the acronym.
- Keep method and baseline names identical in text, tables, and figures.
- Build a quick term list while editing and flag inconsistencies to the user.

## Sentences
- Active voice when the actor matters ("We train", "The model predicts"); passive is fine when the actor is irrelevant ("Samples were filtered").
- Subject and verb close together, early in the sentence; put new or important information at the end.
- Split sentences over ~35 words or with more than two clauses.
- Parallel structure in lists.
- Avoid stacked noun phrases ("graph neural network based node representation learning method").

## Paragraphs
- One idea per paragraph; the first sentence states it.
- Paragraphs that make two points get split; paragraphs that make no point get cut or merged.
- Transitions show logic ("Because", "However", "As a result"), not decoration ("Furthermore" on every sentence).

## Do / Don't
- Don't: "It is worth noting that our approach plays a crucial role in significantly enhancing robustness."
- Do: "Our approach reduces the accuracy drop under noise from 12 to 4 points."

## Working in LetsWritePaper
- Call `section_context {path, section}` before editing; respect open review comments tagged `writing` or `clarity`.
- Make many small `edit_document {path, old_text, new_text, rationale}` calls, each with an exact, unique `old_text`
  of one or two sentences; do not rewrite whole paragraphs or files in one call.
- Use `propose_edit` instead when a change might alter meaning or when the user asked for suggestions.
- Never add citations during an editing pass.
