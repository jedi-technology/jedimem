# The monitor agent's runtime: can jedimem avoid asking for an API key?

**Status:** VERIFIED by direct measurement on this machine, 2026-08-24.
**Host:** Claude Code 2.1.241, `claude -p` headless mode.

## Why this question decides the product

jedimem's install promise is *"install it and you're good to go."* Every config
field we require is a step where adoption leaks — and an API key is the worst
kind of field, because it also creates a secret-handling obligation (see
`05-distribution-security.md`).

The alternative is to borrow the host agent's already-authenticated headless
mode as the extraction runtime: the user is *already* logged into Claude Code /
Codex / pi, so a subprocess inherits that auth for free. Zero config, zero
secrets, and the memory agent's cost lands on the same bill the user already
accepted.

That works. It is also **~30x more expensive per call than it looks**, and the
reason is measurable.

## Measurement 1 — the floor

The cheapest possible headless call: no tools used, three-token answer.

```bash
time claude -p --model claude-haiku-4-5-20251001 --output-format json \
  "Reply with exactly: ok" < /dev/null
```

| Metric | Value |
|---|---|
| `input_tokens` | 10 |
| `cache_creation_input_tokens` | 9,402 |
| `cache_read_input_tokens` | 18,140 |
| `output_tokens` | 43 |
| **billed tokens** | **27,595** |
| **`total_cost_usd`** | **$0.020843** |
| `duration_ms` (API) | 1,412 |
| wall clock | 5.7 s |

**It costs 2 cents and 27.5k tokens to say "ok".** None of that is our payload.
It is Claude Code's own system prompt plus the full tool-definition block, which
is assembled and billed whether or not the tools are ever used.

## Measurement 2 — a real extraction

A 3-line transcript excerpt containing two genuine team conventions:

```
User: no, don't use axios here — we standardized on the internal `httpClient`
wrapper because axios bypasses our retry/tracing middleware.
Assistant: Understood, switching to httpClient.
User: also the integration tests need DATABASE_URL pointing at the docker
compose pg, not the local one.
```

| Metric | Value |
|---|---|
| billed tokens | 21,213 |
| `total_cost_usd` | $0.0223234 |
| memories extracted | 2 (both correct) |
| wall clock | 14.4 s |

Compare to the floor: **the actual extraction work cost $0.0015 on top of a
$0.0208 fixed overhead — 93% of the spend was harness tax.**

`--disallowedTools "Bash,Read,Write,..."` did **not** reduce
`cache_creation_input_tokens`. Denying a tool removes the *permission*, not the
*definition*; the schema is still sent and still billed. VERIFIED.

## Consequences for the design

**1. Batch. Never extract per turn.** This is the single most important
consequence. Per-turn extraction at ~120 human turns/day would cost
120 x $0.021 = **$2.52/day in pure overhead** for maybe $0.18 of real work.
Batching the same day into 8 windowed calls costs **$0.17/day** and loses
nothing, because memory extraction has no latency requirement — it is a
background retrospective, not an inline lookup.

This is also *why* the design has a separate monitor process rather than doing
extraction inside a hook. A hook fires per event; a daemon chooses its own
window. The economics force the architecture.

**2. Always redirect stdin.** Without `< /dev/null`:

```
Warning: no stdin data received in 3s, proceeding without it.
```

`claude -p` blocks for a 3-second stdin timeout when stdin is neither a TTY nor
redirected — exactly the condition inside a daemon or a hook. That is 3 wasted
seconds per call and a hang risk if the parent holds the pipe open. Every
invocation jedimem makes must redirect stdin explicitly. VERIFIED.

**3. Prose-wrapped output; a tool schema is not optional.** With
`--output-format json`, the envelope is JSON but `result` is a *string*
containing a fenced block:

```
"result": "```json\n[\n  {\n    \"kind\": \"requirement\", ...\n```"
```

So parsing needs fence-stripping and must tolerate prose on either side. Worse,
without an enforced schema the model free-styles field types: we asked for
`confidence` and got the string `"high"`, not a number. And it typed
"use httpClient, not axios" as `requirement` when the taxonomy calls it a
`convention` — kind confusion is the default, not the exception.

Both are fixed the same way: force a tool call with a real JSON Schema rather
than asking for JSON in prose. Validation belongs at the tool-call layer where a
mismatch triggers a model retry, not in our parser where it triggers a dropped
memory.

**4. Two runtime modes, and the cheap one is opt-in.**

| Mode | Auth | Cost/call | Config needed |
|---|---|---|---|
| **Host-agent headless** (default) | inherits host login | ~$0.021 fixed + payload | **none** |
| **Direct API** (opt-in) | `ANTHROPIC_API_KEY` | payload only, ~30x cheaper | one env var |

Default to zero-config and let cost-sensitive or high-volume users opt into the
direct path. Do not make the key mandatory — the default must work with no
secrets at all, because that default is what gets jedimem installed.

**5. Model choice is ours to make.** `--model` is honored in headless mode, and
extraction is a cheap classification task. Haiku is the right default; the fixed
overhead dominates anyway, so paying for a larger model buys little on the
extraction step and much on the *consolidation* step, where supersession
decisions are made. Split the two and price them differently.

## Cross-agent generalization

The same borrowed-auth trick should apply to `codex exec` and pi's headless
mode, and the same overhead question applies to each — a headless coding agent
carries its whole harness into every call. Those are measured in
`02-codex.md` and `03-pi.md`. The portable conclusion is the shape, not the
number: **any host-agent-as-runtime design must batch, because the harness tax
is per-invocation and dwarfs the payload.**

## Unverified

- Whether a future `--no-tools` / minimal-harness flag exists that would cut the
  fixed cost. Nothing in `--help` suggests one at 2.1.241.
- Whether the 1-hour prompt cache (`ephemeral_1h_input_tokens: 9402` observed)
  makes back-to-back batched calls materially cheaper than the floor suggests.
  If it does, batching wins by even more than computed above. Worth measuring
  before setting the batch window.
