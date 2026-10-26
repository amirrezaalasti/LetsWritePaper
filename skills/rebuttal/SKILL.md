---
name: rebuttal
description: Respond to review comments in LetsWritePaper — triage open comments, make or propose revisions linked to each comment, record responses with a status, and draft a rebuttal letter within the venue's limits. Use when the user wants to address reviews, revise after reviewing, or write an author response.
kind: review
sections: []
---

# Responding to Reviews

The goal is to improve the paper where reviewers are right, and to convince them (and the area chair) where they are not —
with evidence, not adjectives.

## Workflow
1. `list_review_comments {status: "open"}` — collect all open comments. Read each with its anchored quote and section.
2. Read the affected sections with `section_context {path, section}` (it also shows the comments anchored there).
3. Triage every comment into one decision:
   - **address** — the reviewer is right, or the change is cheap and harmless. Fix it.
   - **dispute** — the reviewer is factually wrong or misread the paper. Respond with evidence; usually still clarify the text,
     since a misreading signals unclear writing.
   - **wontfix** — valid but out of scope, infeasible before the deadline, or in conflict with another reviewer. Explain why
     and, if possible, acknowledge it as a limitation.
4. Prioritize: all `major` comments first, then `question`, then `minor`, then `nit`. Group duplicates across reviewers.
5. Revise:
   - `edit_document {path, old_text, new_text, rationale}` for clear fixes (typos, clarifications, added details).
     `old_text` must be an exact, unique excerpt; keep edits small and targeted.
   - `propose_edit` for substantive changes (claims, framing, new results, restructuring) or when the user asked for
     suggestions; the user decides.
   - The `rationale` must reference the comment id ("Addresses comment #c12: define the noise model before Eq. 3").
6. `respond_to_comment {comment_id, response, status}` with `status` exactly one of `addressed`, `disputed`, `wontfix`.
   - The response quotes or summarizes the change made and says where: "Added in Section 4.2: 'We tune all baselines with
     the same 50-trial budget.'"
   - Never mark `addressed` unless the edit was actually made (or proposed, and say so).

## Never
- Fabricate results, numbers, or experiments to satisfy a reviewer. If a requested experiment is needed, write
  `TODO: run <experiment>` in a proposed edit and tell the user.
- Invent citations. Cite only keys in the project's .bib files (check with `read_document`).
- Silently weaken or delete a claim without telling the user.
- Promise changes in a response that are not made.

## Rebuttal letter tone
- Thank briefly once, at the top; not in every paragraph.
- Polite, direct, factual. No sarcasm, no "the reviewer clearly misunderstood".
- Lead each response with the answer, then the evidence: "Yes — we added ... (Table 3)" or
  "We respectfully disagree: Section 3.1 states ..., and Table 5 shows ...".
- Concede valid points plainly; it buys credibility for the points you dispute.
- Quote the revised text or give the new number rather than describing it vaguely.
- Address the area chair implicitly: make it easy to see that major concerns were resolved.

## Structure of a rebuttal letter
1. Short general response: thanks, a summary of the main changes, and responses to concerns shared across reviewers.
2. Per reviewer, per comment: a one-line paraphrase of the concern (with comment id), then the response and the change.
3. Order by importance, not by the order the reviewer wrote them.

## Venue limits
- Rebuttal limits vary by venue and year — character limits per reviewer (often a few thousand characters),
  page limits for a response PDF, or a fixed number of discussion posts. Ask the user for the exact limit and check
  length before finalizing.
- Under tight limits: answer `major` comments and factual errors first; batch nits into a single line
  ("We fixed all typos and formatting issues noted").
- Some venues forbid new experiments or links in rebuttals; check the rules.
- Journals: produce a point-by-point response document that quotes each comment in full and gives location
  (page/section) of each change; marking changes in the revised manuscript is often required.

## Checklist
- [ ] Every open comment has a decision and a `respond_to_comment` call.
- [ ] Every `addressed` status corresponds to a real edit or proposal, referenced by comment id.
- [ ] Changes made for one comment do not contradict claims elsewhere (re-check abstract, intro, conclusion).
- [ ] Rebuttal within the venue's length limit.
