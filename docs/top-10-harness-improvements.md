# Top 10 improvements to the Grok Build coding harness

This is a ranked list of changes that would most improve **coding-agent reliability** — completing real software work without losing context, clobbering files, or silently sharing a workspace. It is not a TUI polish list.

The harness is already mature: parallel tool dispatch with same-file serialization, structured edit errors, path suggestions, a goal planner/implementer/verifier loop, two-pass compaction (opt-in), and hashline-anchored edits. The gaps below are where those pieces are **off by default, fail open, or hidden from the model**.

Impact is judged by how often a failure kills a long coding session, not by how hard the change is.

---

## 1. Make compaction preserve agency

**Why it is first.** Long coding sessions die at compaction more often than they die at the first edit. Grok Build uses **full-replace** compaction: the conversation is summarized into nine sections and the earlier turns are discarded. Defaults then strip the successor of the identity it needs to continue:

- Auto-compact fires at **85%** of the context window (`CompactionPolicy` in `crates/codegen/xai-grok-agent/src/compaction.rs`).
- **Two-pass compaction is off by default** (`two_pass_enabled: false`). The better path already exists (`build_two_pass_compaction_prompt` in `crates/codegen/xai-grok-shell/src/session/helpers/session_compact.rs`) but real sessions keep the single-pass full-replace unless config opts in.
- **Memory flush before compact is also off** (`memory_flush_enabled: false`), so durable facts are not written out before the transcript is thrown away.
- After compact, the system prompt collapses to two sentences (`COMPACT_SYSTEM_PROMPT` in `crates/codegen/xai-grok-agent/src/prompt/template.rs`). Work-policy, tool conventions, and verification discipline do not come back unless a reminder happens to reinject them.

The nine-section summary prompt asks for full snippets of edited code and *all* user messages, then tells the model to stay economical. Degenerate or truncated summaries are the usual failure: lost file paths, a wrong “current work,” invented pending tasks.

**What to do.**

- Default-on two-pass compaction, and keep the prefire fingerprint so a stale pass-1 note is not reused.
- Default-on memory flush when memory is enabled.
- After compact, reinject a short work-policy block (tool names, edit contract, “do not stop with open todos”) instead of the two-line stub.
- Validate summaries before swapping history: require non-empty sections 1, 3, 7, and 8; reject “None” for Current Work when the pre-compact turn had tool calls.
- Treat `/tmp/compaction/segment_*.md` (already mentioned in the prompt) as a real out-of-band store the successor is allowed to read, not only as a “do not touch” note.

---

## 2. Make file edits fail-safe by default

`search_replace` is the primary GrokBuild edit tool. Three defaults are the opposite of what a coding agent needs:

| Setting | Default | Effect |
|---|---|---|
| `empty_old_string_does_not_override` | `false` | Empty `old_string` **overwrites the whole file**. The guard sentence is omitted from the tool description unless this is opted in. |
| `unicode_normalized_fallback` | `false` | Smart quotes / em-dashes from the model vs the file cause `NoMatchesFound` death spirals. |
| `skip_read_before_edit` | documented as requiring a Read first | **Runtime no-op.** It only gates “is a Read tool in the toolset?” at config time. |

See `SearchReplaceParams` in `crates/codegen/xai-grok-tools/src/implementations/grok_build/search_replace/mod.rs`.

Exact-substring uniqueness is already the right shape (nearest-match hints, `replace_all`, CRLF restore). The damage comes from silent overwrite and from retries that never re-read.

**What to do.**

- Default `empty_old_string_does_not_override = true`. Keep a dedicated `write` (see #6) for intentional full-file creates.
- Default `unicode_normalized_fallback = true` when the normalized match is unique; keep byte-exact as the first try.
- Enforce read-before-edit **per file, per session**: refuse `search_replace` if that path has not been read (or hashline-read) since the last mutation or since session start, unless the caller sets an explicit force flag.
- On `NoMatchesFound`, always include the nearest snippet *and* a “re-read this path” instruction; the user-edit hint already exists and should stay on.

---

## 3. Make the edit/read line contract unambiguous — or ship hashline as default

`read_file` numbers only the first line and **every 10th line** (`LINE→CONTENT`). The description tells the model to count from the nearest anchor (`DESCRIPTION_FULL` in `crates/codegen/xai-grok-tools/src/implementations/grok_build/read_file/mod.rs`). Off-by-N edits follow directly: the model quotes the wrong unique string, or invents line numbers that never appeared.

The **hashline** pack (`crates/codegen/xai-grok-tools/src/implementations/grok_build_hashline/`) already solves this: content hashes on anchors, stale-anchor recovery, atomic bottom-up batches, overlap rejection, and detection of pasted `LINE:HASH→` prefixes. It is a separate namespace that the registry **refuses to mix** with standard file tools.

So the harness has a better editor that most sessions never see, and a default reader that makes the worse editor harder to use.

**What to do.**

- Short term: number **every** returned line in GrokBuild `read_file` (hashline can keep sparse hashed anchors). Token cost is small next to a failed edit + re-read loop.
- Medium term: make hashline the default coding toolset (read/edit/grep) and keep exact `search_replace` as a fallback for unique-string edits the model already has in context.
- Either way, stop describing two different line prefixes (`→` vs OpenCode `": "`) in the same product.

---

## 4. Stop letting the model end the turn with unfinished work

The loop already has the right machinery and then leaves it off:

- **TodoGate** inspects pending / unbacked in-progress todos after a content-only assistant message and forces another turn. It is **disabled by default** (`TodoGateConfig` in `crates/codegen/xai-grok-agent/src/system_reminder.rs`). Operators opt in with remote `todo_gate_enabled` or `--todo-gate`. Cap is only two fires per prompt when it *is* on.
- **Action stationarity** nudges after **8** identical tool calls and hard-stops at **16** (`turn.rs`). For `get_task_output` / `true` no-ops that is a lot of wasted context; true-noop already stops at 4, but polling tools do not get that tighter cap.
- The **laziness classifier** is aimed at goal mode. Ordinary coding sessions that say “done” with open todos or unrun tests are not gated the same way.

**What to do.**

- Enable TodoGate by default for primary coding agents (not for tiny one-shot `-p` asks). Keep the fire cap, but raise it slightly for goal mode.
- Stationarity: nudge at 3 identical `get_task_output` / sleep / `true` calls; keep 8/16 for other tools.
- At turn end, if the todo list has `in_progress` or `pending` items and there is no running background task backing them, inject one reminder rather than emitting `TurnOutcome::Completed`.

---

## 5. Fail closed when a subagent asked for isolation and did not get it

Subagents are the right way to keep the parent context small. Isolation is **optional worktree**, and on failure the child **silently shares the parent workspace**:

```text
Failed to create worktree, falling back to shared workspace
Failed to rehydrate subagent worktree, falling back to shared workspace
```

See `crates/codegen/xai-grok-shell/src/agent/subagent/handle_request.rs`. The parent is not told in model-visible text. Two implementer children can then race the same files; hunk tracking will show a mash-up as if it were intentional.

Admission already caps nesting depth (default **1**) and concurrency (**32**). That is necessary but not sufficient if isolation is a lie.

**What to do.**

- If `SubagentIsolationMode` is not `None` and worktree create/rehydrate fails, return a tool error to the parent. Do not start the child on the shared cwd.
- Surface admission rejections (queue vs reject, depth, fd cap) in the `task` tool result, not only in logs.
- When isolation succeeds, make the completion payload include the worktree path and a file-change summary so the parent can merge deliberately instead of polling `get_task_output` for prose.

---

## 6. Show the model the search and write primitives it already has

GrokBuild’s advertised file tools are `read_file`, `search_replace`, `list_dir`, `grep`. Two holes push the model into shell:

- **`grep.output_mode` is accepted on the wire and omitted from the JSON schema** (`#[schemars(skip)]` in `crates/codegen/xai-grok-tools/src/implementations/grok_build/grep/mod.rs`). Files-with-matches and count modes exist (defaults 200 content / 500 files) but the model cannot discover them, so it greps for content and blows the 40KB cap, or runs `rg --files`.
- There is **no first-class GrokBuild `write` or `glob`**. Creates go through empty-`old_string` `search_replace` (see #2). Recursive file find goes through `list_dir` + `grep` or bash. OpenCode *does* ship `write` and `glob`.

**What to do.**

- Put `output_mode` (`content` | `files_with_matches` | `count`) in the schema with the real caps.
- Add GrokBuild `write` (create or overwrite with an explicit flag) and `glob` (or a `list_dir` recursive/glob param). Then empty `old_string` can stop meaning “truncate this file.”
- Keep recovery footers consistent: every truncated tool result should name the log path and the exact follow-up tool (`read_file` / `grep` / `get_task_output`), the way bash already does.

---

## 7. Freeze one coding tool pack; stop shipping five dialects

The registry knows about **GrokBuild**, **GrokBuildConcise**, **GrokBuildHashline**, **OpenCode**, and **Codex** (`apply_patch`). Mixing standard file tools with hashline is rejected, which is good. Everything else can still coexist: camelCase vs snake_case params, `→` vs `": "` line prefixes, different overwrite semantics, concise descriptions that drop the overwrite-guard sentence.

Fixes then land in one variant. Eval and prompt text drift. `ToolKind::Other` on scheduler tools already blocks template binding (`scheduler/create.rs`), so even the “live tool names in the prompt” system has holes.

**What to do.**

- Pick a **default coding pack** (GrokBuild+hashline, or GrokBuild after #3/#6) and treat the others as compatibility shims with a single shared implementation.
- Drive all descriptions through `TemplateRenderer` + `ToolKind` so a rename cannot desync prompt vs schema. Give scheduler/workflow real kinds.
- Add a registry test: every `ToolKind` used in `prompt.md` / `subagent_prompt.md` has a live tool in the default pack.

---

## 8. Make mid-session mode switches actually update the agent

`Agent::update_policies_from_definition` is a documented no-op:

```rust
// TODO: completion requirements and retry configs are now part of
// ToolServerConfig and handled at registry finalization time.
// Mid-session policy updates are not yet supported in the new architecture.
```

(`crates/codegen/xai-grok-agent/src/agent.rs`)

Plan → agent, tool overrides, and completion requirements (`complete_task`, retry/backoff) are therefore whatever was baked at session start. `finalize_prompt` can re-render text; it does not rebuild completion gates or retry policy. That is how a session can *say* it left plan mode and still lack write tools, or keep nagging for a completion tool that is gone.

**What to do.**

- Implement the stub: rebuild completion requirement, retry config, and plan-mode tool filter from the new definition without dropping chat state.
- On plan/agent/ask switches, emit a model-visible reminder of the *current* tool set (write/edit allowed or not) so the prompt and the registry cannot disagree.
- Cover this with a session test: open in plan mode, switch to agent, assert `search_replace` is callable and the completion tool matches the new definition.

---

## 9. Close the goal loop with structured verifier output, not more prose

The goal harness is already a serious coding-eval loop: planner, implementer, adversarial verifier, and a strategist that fires when the implementer is stuck in whack-a-mole (`session/templates/goal_*.md`). The implementer is told to seed todos from acceptance criteria and to write honest tests. The verifier returns `findings[]` with `kind` = `bug` | `gap` | `todo`.

Two things keep it from converging:

- Findings are inlined as chat prose. The implementer can “address” them in narration without turning them into `todo_write` items or re-running the named test.
- The verifier is instructed not to rebuild the implementer’s tests, only to audit them. If the implementer never recorded evidence under the scratch dir, the next round starts from vibes.

**What to do.**

- Parse verifier `findings` into the todo list automatically (one item per finding, `in_progress` on the first). Do not rely on the implementer to copy them.
- Require each finding to name a command or a file:line the next implementer round must run or edit. Reject empty `location`.
- After a strategist note, pin **one** structural change as the sole `in_progress` todo and drop the rest of the mole-whacking list for that round.
- Keep the honest-test rules; enforce them with a cheap static check (no `todo!()`, no `#[ignore]` on tests this goal added) before the LLM verifier runs.

---

## 10. Make truncated tool results and user questions recoverable

Coding loops stall when the model cannot see what happened, or thinks it asked the user and got an answer.

- Bash already middle-truncates at ~20k chars and returns a log path. MCP truncation dumps overflow at **20KB** (`util/mcp_truncate.rs`) with a weaker recovery story.
- Concise-mode bash still has no distinct message when a foreground command is auto-backgrounded (`grok_build_concise/bash.rs`).
- `ask_user_question` still has a **fire-and-forget fallback** when `UserQuestionSender` is not injected (`AskUserQuestionTool::fallback_fire_and_forget`). The model receives `QuestionsSent` and continues as if the user answered.
- Empty tool-call ids are synthesized as `missing-call-id-{idx}`, which breaks later `get_task_output` correlation.

**What to do.**

- One truncation footer for every tool: bytes kept, total bytes, path to the full log, and the exact tool+args to read the rest.
- Delete the ask-user fire-and-forget path; if the sender is missing, return a tool error, do not pretend the question was asked.
- Reject missing tool-call ids from the sampler instead of synthesizing them.
- When auto-backgrounding on timeout, say so in `prompt_text` with the task id and the `get_task_output` / `kill_task` names from the live toolset.

---

## What this list deliberately skips

TUI, theming, dashboard, Arabic/Persian reordering, mouse selection, and most changelog 1.0.x polish. Those matter for the product; they do not change whether the agent finishes a refactor after the third compaction.

Auth-retry and stream-drain races (`sampler_turn.rs`) abort otherwise good turns and are worth a follow-up, but they are infrastructure. The ten items above are the harness’s **coding contract** with the model: context, edits, turn-end, isolation, tools, mode switches, goals, and recoverable I/O.

---

## Suggested order of work

1. Defaults that are one-flag / one-default flips: TodoGate on, overwrite guard on, unicode fallback on, `output_mode` in grep schema, every-line read anchors, fail-closed worktrees.
2. Compaction successor prompt + two-pass / memory-flush defaults + summary validation.
3. First-class `write`/`glob` and hashline-as-default (or a documented single pack).
4. `update_policies_from_definition` actually updating, then structured goal findings → todos.
5. Truncation footer unification and removal of ask-user fire-and-forget.
