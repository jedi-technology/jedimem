# Security

jedimem commits files into your repo, ships a hook that executes on your machine,
and reads repo files into an AI agent's context. That is a genuinely hostile
surface. This document states what we mitigate and what we do not.

## Reporting

Open a private security advisory on the GitHub repository. Do not open a public
issue for a vulnerability.

## Threat model

| # | Threat | Vector | Mitigation | Residual risk |
|---|---|---|---|---|
| 1 | **Supply chain** | a malicious commit upstream runs at every teammate's session start | pinned version refs, never a floating branch; content-hash pinned hook trust; no post-install scripts | **Real.** Pinning delays compromise, it does not prevent it. A user who updates gets the new code. |
| 2 | **Repo-borne code execution** | a hostile PR edits `.jedimem/bin/*` or the hooks file; the next teammate's agent session executes it | hook trust pinned by SHA-256 in the *user's* config (the model Codex ships), so any edit revokes trust until re-approved; `CODEOWNERS` on `.jedimem/**`; hooks confined to one script path | **Real.** A reviewer who approves the PR defeats every control. This is the central risk of an in-repo design. |
| 3 | **Prompt injection via memory** | a memory file is read into agent context, so a malicious memory is an instruction with persistence across every future session | human review gate before anything is shared; memories are data and can never grant capabilities or tool permissions; provenance on every memory; `jedimem contest` | **Real and unsolved industry-wide.** We reduce the blast radius; we do not eliminate it. |
| 4 | **Secret capture** | a token in a transcript becomes a committed memory, and git history keeps it forever | redaction runs before *any* write, including to the staging ref; tool payloads are digested, never captured verbatim; `.jedimem/local/` is never committed | Redaction is best-effort pattern matching. A leaked secret requires history rewrite plus rotation — rotation is the real remedy. |
| 5 | **Surveillance** | memory becomes a performance-review artifact | the extraction schema forbids a person as a memory's subject; personal preferences stay local and untracked; `jedimem pause`; no telemetry | Social, not technical. A determined manager can read git history regardless. |
| 6 | **Context exfiltration** | the extraction runtime sends transcript content to a model provider | uses the provider your agent is already talking to; no third-party endpoint; no jedimem server exists | Your transcripts already go to that provider. jedimem adds no new recipient, but it does send *more* of them. |

## What we refuse to build

- No server, no account, no telemetry — there is nothing to exfiltrate to.
- No post-install scripts, and no `curl | sh` as the recommended path.
- No network access on the install path.
- No writes outside `.jedimem/`, the two compiled instruction sections, and the
  side ref.
- No memory that can grant a capability, permission, or tool access.
- No automatic PRs, and no automatic commits to a checked-out branch.

## Hardening checklist for adopters

1. Add `CODEOWNERS` covering `.jedimem/**` and require review.
2. Pin the jedimem version; review the diff before updating.
3. Run `jedimem lint` in CI to reject schema-invalid memories.
4. Treat a memory diff as a code diff, because it is an instruction diff.
5. Rotate any credential you believe entered a transcript. Do not rely on
   redaction retroactively.
