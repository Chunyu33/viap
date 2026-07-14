---
name: skills-repo-release-docs
description: Use this whenever the user adds a new skill, updates/fixes an existing skill's content, or asks to "update the docs" / "prepare a release" / "bump version" in this skills repository. Automatically updates README.md's skill list and CHANGELOG.md to match whatever style already exists in this repo — do not ask the user to re-explain the desired format, infer it from the existing files. Trigger even on terse requests like "把这个新 skill 加进文档里" or "文档更新一下" without the user re-describing the workflow each time.
---

# Skills Repo Release Docs

This skill automates the repetitive part of maintaining this skills repository's `README.md` and `CHANGELOG.md` — so the user doesn't have to type the same instructions every time they add or update a skill. **It does NOT commit, tag, or push** — the user reviews the diff and handles git themselves, same as before.

## Step 1 — Learn the existing style before writing anything

Never assume a format. Read the current `README.md` and `CHANGELOG.md` in the repo root first, and infer:

- **README**: how the skill list/table is structured (columns, language, link format), where in the file it lives, what other sections exist and their order.
- **CHANGELOG**: which changelog convention is in use (Keep a Changelog style, plain bullet list, etc.), the exact heading format for a version section (e.g. `## [1.0.0] - 2026-06-19`), whether there's an `[Unreleased]` section, what subsection labels are used (`Added`/`Changed`/`Fixed` or something else), and the language/tone of existing entries.

If either file doesn't exist yet, ask the user once before inventing a structure — but if it exists, just match it silently, don't ask "what format do you want."

## Step 2 — Figure out what changed

Look at the conversation/diff to determine the nature of the change:

- **New skill added** → README gets a new row/entry in the skill list; CHANGELOG gets an `Added` entry.
- **Existing skill content fixed or expanded** (corrected a wrong claim, added a newly-discovered bug pattern, etc.) → CHANGELOG gets a `Fixed`/`Changed` entry; README only changes if the skill's one-line description changed.
- **Breaking change to skill format/structure itself** → flag this explicitly to the user, since it usually means a MAJOR version bump under semver.

## Step 3 — Update README.md

Add/update the entry for the relevant skill in whatever list/table format already exists, matching column structure, language, and link style exactly. Keep entries alphabetically or chronologically ordered consistent with how existing entries are ordered — check before assuming.

## Step 4 — Update CHANGELOG.md

**Critical rule, always enforce this regardless of what the user asks for in the moment**: never edit content under a version heading that has already been published (i.e. already has a date and doesn't say "Unreleased" — a published version is treated as immutable history, even if the user's wording sounds like they want it edited in place).

- If an `[Unreleased]` section exists: add the new entry there, under the matching subsection label (`Added`/`Fixed`/etc., matching whatever labels this repo already uses).
- If no `[Unreleased]` section exists in this repo's convention: ask once whether to create one, or whether this repo's convention is to go straight to a new version number on every change — then follow whatever the user says for all future runs of this skill in this repo (don't re-ask next time; infer from what's now in the file).
- **Do not pick a version number yourself.** Determining MAJOR/MINOR/PATCH and actually cutting the version section is a deliberate publish step the user does separately (see Step 5) — this skill's job ends at getting the change recorded under Unreleased (or wherever this repo's convention puts pending changes).

## Step 5 — Stop short of publishing

This skill prepares the docs for review. It does not:
- choose or write a new version number / heading
- run `git commit`, `git tag`, or `git push`
- touch `.github/workflows/release.yml`

After making the README/CHANGELOG edits, summarize what was changed in 2-3 sentences and let the user review the diff themselves. If the user separately says something like "cut a release" / "打个新版本", treat that as the trigger to move the Unreleased content into a new version heading following this repo's existing numbering scheme (semver unless the existing CHANGELOG shows otherwise) — but that's a distinct action from documenting the change itself, do these as two separate, clearly-announced edits even if requested in the same message.
