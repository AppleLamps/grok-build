//! Agent — a fully built agent: definition + session context.

use std::sync::Arc;

use xai_grok_sampling_types::HostedTool;
use xai_grok_tools::bridge::ToolBridge;
use xai_grok_tools::types::definition::ToolDefinition;

use crate::compaction::CompactionPolicy;
use crate::config::{AgentDefinition, CompletionRequirement, PermissionMode};
use crate::prompt::context::PromptContext;
use crate::system_reminder::ReminderPolicy;

/// A fully built agent: definition + session context.
///
/// NOT portable — tied to a specific session via its ToolBridge,
/// rendered system prompt, and session-level policies.
///
/// Created by AgentBuilder from an AgentDefinition + session context.
///
/// The Agent is session-bound after construction. Chat state lives on
/// the host; this type holds `Arc<ToolBridge>` and a definition that
/// mid-session mode switches may overlay without rebuilding the
/// registry. Mutations to tool state (MCP registration, completion
/// tracking) go through ToolBridge's internal locks.
pub struct Agent {
    /// The definition currently in effect (may be a session-mode overlay).
    definition: AgentDefinition,

    /// Definition the session was built from (or last named-agent switch).
    /// Plan/ask/agent overlays restore from this so a mode toggle cannot
    /// drop the home completion requirement or permission mode.
    base_definition: AgentDefinition,

    /// The context that produced the current system prompt.
    /// Stored for inspection, re-rendering, and serialization.
    prompt_context: PromptContext,

    /// The rendered system prompt (cached from prompt_context.render()).
    system_prompt: String,

    /// The tool bridge — owns ToolRegistry + ToolState + SessionContext.
    tool_bridge: Arc<ToolBridge>,

    /// Session-level policies.
    reminder_policy: ReminderPolicy,
    compaction_policy: CompactionPolicy,

    /// Backend-hosted tools to include in API requests.
    /// These are sent as native Responses API types (e.g., `WebSearch`)
    /// and executed server-side by the agentic sampler.
    hosted_tools: Vec<HostedTool>,

    /// Build-time toggle for server-side search tools. ANDed at request
    /// time with the per-model `SessionActor::supports_backend_search`.
    backend_search_enabled: bool,
}

impl Agent {
    /// Create a new Agent.
    ///
    /// Normally called by `AgentBuilder::build()`. Exposed publicly for
    /// test helpers that need to construct an Agent with a pre-built ToolBridge.
    pub fn new(
        definition: AgentDefinition,
        prompt_context: PromptContext,
        system_prompt: String,
        tool_bridge: Arc<ToolBridge>,
        reminder_policy: ReminderPolicy,
        compaction_policy: CompactionPolicy,
        hosted_tools: Vec<HostedTool>,
        backend_search_enabled: bool,
    ) -> Self {
        Self {
            base_definition: definition.clone(),
            definition,
            prompt_context,
            system_prompt,
            tool_bridge,
            reminder_policy,
            compaction_policy,
            hosted_tools,
            backend_search_enabled,
        }
    }

    // ── From definition ──────────────────────────────────────────────

    /// Agent name (unique identifier).
    pub fn name(&self) -> &str {
        &self.definition.name
    }

    /// Agent description.
    pub fn description(&self) -> &str {
        &self.definition.description
    }

    /// The full agent definition currently in effect.
    pub fn definition(&self) -> &AgentDefinition {
        &self.definition
    }

    /// Home definition this session was built from (or last named-agent
    /// switch). Session-mode overlays restore from this.
    pub fn base_definition(&self) -> &AgentDefinition {
        &self.base_definition
    }

    /// Permission mode for this agent.
    pub fn permission_mode(&self) -> &PermissionMode {
        &self.definition.permission_mode
    }

    /// Completion requirement, if any.
    pub fn completion_requirement(&self) -> Option<&CompletionRequirement> {
        self.definition.completion_requirement.as_ref()
    }

    // ── Session-level ────────────────────────────────────────────────

    /// The rendered system prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Compact system prompt for post-compaction use.
    ///
    /// Returns a static string — the compact prompt never changes at runtime.
    pub fn compact_system_prompt(&self) -> &str {
        crate::prompt::template::COMPACT_SYSTEM_PROMPT
    }

    /// The tool bridge for this agent.
    pub fn tool_bridge(&self) -> &Arc<ToolBridge> {
        &self.tool_bridge
    }

    /// Compaction policy.
    pub fn compaction_policy(&self) -> &CompactionPolicy {
        &self.compaction_policy
    }

    /// Reminder policy.
    pub fn reminder_policy(&self) -> &ReminderPolicy {
        &self.reminder_policy
    }

    /// Cached AGENTS.md section (derived from prompt_context).
    pub fn agents_md_section(&self) -> Option<String> {
        self.prompt_context.format_agents_md_section()
    }

    /// AGENTS.md content formatted for user-message injection.
    ///
    /// Returns the `<system-reminder>` block to prepend as a user message,
    /// respecting audience (compacted for subagents) and template.
    pub fn agents_md_user_reminder(&self) -> Option<String> {
        self.prompt_context.agents_md_user_reminder()
    }

    /// Personas content formatted for user-message injection.
    ///
    /// Returns the `<system-reminder>` block to prepend as a user message,
    /// respecting audience (suppressed for subagents) and template.
    pub fn personas_user_reminder(&self) -> Option<String> {
        self.prompt_context.personas_user_reminder()
    }

    /// The structured prompt context for inspection and re-rendering.
    pub fn prompt_context(&self) -> &PromptContext {
        &self.prompt_context
    }

    /// Audience this agent's prompt was rendered for (Primary or Subagent).
    ///
    /// Used by the runtime turn-end TodoGate: the gate runs for the
    /// primary audience and is suppressed for subagents.
    pub fn prompt_audience(&self) -> crate::prompt::context::PromptAudience {
        self.prompt_context.audience
    }

    /// Tool definitions for the sampling API — delegates to ToolBridge.
    pub async fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tool_bridge.tool_definitions().await
    }

    /// Backend-hosted tools that should be included in API requests.
    /// These are sent as native types (e.g., `rs::Tool::WebSearch`) and
    /// executed server-side by the agentic sampler.
    pub fn hosted_tools(&self) -> &[HostedTool] {
        &self.hosted_tools
    }

    /// Build-time toggle for server-side search tools. Callers should
    /// AND this with the per-model `supports_backend_search` flag to
    /// decide whether to ship `hosted_tools` on a request. Do not use
    /// `hosted_tools().is_empty()` as a proxy — the list also depends
    /// on web-search config.
    pub fn backend_search_enabled(&self) -> bool {
        self.backend_search_enabled
    }

    /// Built-in tool definitions only (excludes MCP tools).
    pub async fn tool_definitions_builtins_only(&self) -> Vec<ToolDefinition> {
        self.tool_bridge.tool_definitions_builtins_only().await
    }

    /// Whether auto-compact should trigger given current token usage.
    ///
    /// `context_window` comes from the session's SamplingConfig (model-provided).
    pub fn should_auto_compact(
        &self,
        total_tokens: u64,
        context_window: std::num::NonZeroU64,
    ) -> bool {
        let cw = context_window.get();
        xai_token_estimation::exceeds_threshold(
            total_tokens,
            cw,
            self.compaction_policy.auto_compact_threshold_percent as u8,
        )
    }

    /// Apply a named-agent definition's runtime policies and make it the
    /// new home. Does **not** rebuild the tool registry or re-render
    /// prompts — the host re-renders the system message separately.
    ///
    /// Used for mid-session switches to a different agent (e.g.
    /// `browser_use`). Plan/ask/agent session-mode toggles should call
    /// [`Self::apply_session_mode_overlay`] so the original home is
    /// restored when leaving the overlay.
    pub fn update_policies_from_definition(&mut self, def: &AgentDefinition) {
        apply_runtime_policies(&mut self.definition, def);
        self.sync_prompt_context_from_definition(def);
        self.base_definition = def.clone();
    }

    /// Overlay plan/ask (read-only, no completion gate) or restore the
    /// home definition for agent mode. Chat state and the tool registry
    /// stay put.
    pub fn apply_session_mode_overlay(&mut self, restore_home: bool) {
        let overlay = if restore_home {
            self.base_definition.clone()
        } else {
            readonly_mode_overlay(&self.base_definition)
        };
        apply_runtime_policies(&mut self.definition, &overlay);
        self.sync_prompt_context_from_definition(&overlay);
    }

    fn sync_prompt_context_from_definition(&mut self, def: &AgentDefinition) {
        self.prompt_context.prompt_mode = def.prompt_mode.clone();
        self.prompt_context.prompt_body = def.prompt_body.clone();
        self.prompt_context.system_prompt = def.system_prompt.clone();
        self.prompt_context.include_browser_verification = def.include_browser_verification();
        if !def.agents_md {
            self.prompt_context.agents_md_files.clear();
        }
    }

    /// Re-render the system prompt from current ToolBridge state
    /// (tool name overrides, disabled tools). Called by hosts after
    /// mid-session tool-override updates.
    pub async fn finalize_prompt(&mut self) {
        self.prompt_context.build_timestamp_utc = chrono::Utc::now().to_rfc3339();

        self.system_prompt = self
            .prompt_context
            .render(&self.tool_bridge)
            .await
            .unwrap_or_default();
    }

    /// Re-render the system prompt for a different definition, reusing
    /// the existing ToolBridge. Used for mid-session mode switching.
    pub async fn render_prompt_for_definition(&self, definition: &AgentDefinition) -> String {
        let mut ctx = self.prompt_context.clone();
        ctx.prompt_mode = definition.prompt_mode.clone();
        ctx.prompt_body = definition.prompt_body.clone();
        ctx.system_prompt = definition.system_prompt.clone();
        ctx.include_browser_verification = definition.include_browser_verification();
        ctx.build_timestamp_utc = chrono::Utc::now().to_rfc3339();

        // Clear agents_md if the new definition doesn't want it
        if !definition.agents_md {
            ctx.agents_md_files.clear();
        }

        ctx.render(&self.tool_bridge).await.unwrap_or_default()
    }
}

/// Copy the fields a mid-session switch is allowed to change without
/// rebuilding the registry. Chat identity (MCP servers, session tool
/// clamps, allowed subagent types) stays on `dst`.
pub(crate) fn apply_runtime_policies(dst: &mut AgentDefinition, src: &AgentDefinition) {
    dst.completion_requirement = src.completion_requirement.clone();
    dst.permission_mode = src.permission_mode.clone();
    dst.prompt_mode = src.prompt_mode.clone();
    dst.tool_overrides = src.tool_overrides.clone();
    dst.prompt_body = src.prompt_body.clone();
    dst.system_prompt = src.system_prompt.clone();
    dst.agents_md = src.agents_md;
    dst.name = src.name.clone();
    dst.description = src.description.clone();
}

/// Plan/ask overlay: read-only permission and no completion gate, cloned
/// from the session's home definition so leaving the overlay can restore
/// the original requirement.
pub(crate) fn readonly_mode_overlay(home: &AgentDefinition) -> AgentDefinition {
    let mut overlay = home.clone();
    overlay.permission_mode = PermissionMode::Plan;
    overlay.completion_requirement = None;
    overlay
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    /// Standalone function testing the same logic as Agent::should_auto_compact
    fn should_auto_compact_check(total_tokens: u64, context_window: u64, threshold: u32) -> bool {
        let cw = NonZeroU64::new(context_window).expect("test context_window must be non-zero");
        let usage_percent = (total_tokens * 100) / cw.get();
        usage_percent >= threshold as u64
    }

    #[test]
    fn test_should_auto_compact_below_threshold() {
        // 80% of 100K window with 85% threshold → false
        assert!(!should_auto_compact_check(80_000, 100_000, 85));
    }

    #[test]
    fn test_should_auto_compact_above_threshold() {
        // 90% of 100K window with 85% threshold → true
        assert!(should_auto_compact_check(90_000, 100_000, 85));
    }

    #[test]
    fn test_should_auto_compact_at_threshold() {
        // Exactly 85% of 100K window with 85% threshold → true
        assert!(should_auto_compact_check(85_000, 100_000, 85));
    }

    #[test]
    fn test_should_auto_compact_empty_usage() {
        // 0 tokens used → false
        assert!(!should_auto_compact_check(0, 100_000, 85));
    }

    #[test]
    fn test_should_auto_compact_100_percent_threshold() {
        // 100% threshold → only triggers when fully used
        assert!(!should_auto_compact_check(99_999, 100_000, 100));
        assert!(should_auto_compact_check(100_000, 100_000, 100));
    }

    #[test]
    fn apply_runtime_policies_copies_completion_and_permission() {
        use super::{apply_runtime_policies, readonly_mode_overlay};
        use crate::config::{AgentDefinition, CompletionRequirement, PermissionMode};

        let mut home = AgentDefinition::default_grok_build();
        home.completion_requirement = Some(CompletionRequirement {
            tool: "complete_task".into(),
            reminder: "Call complete_task before ending.".into(),
            recovery: None,
        });
        home.permission_mode = PermissionMode::BypassPermissions;

        let overlay = readonly_mode_overlay(&home);
        assert_eq!(overlay.permission_mode, PermissionMode::Plan);
        assert!(overlay.completion_requirement.is_none());
        assert_eq!(overlay.name, home.name);

        let mut live = home.clone();
        apply_runtime_policies(&mut live, &overlay);
        assert!(live.completion_requirement.is_none());
        assert_eq!(live.permission_mode, PermissionMode::Plan);

        apply_runtime_policies(&mut live, &home);
        assert_eq!(
            live.completion_requirement.as_ref().map(|c| c.tool.as_str()),
            Some("complete_task")
        );
        assert_eq!(live.permission_mode, PermissionMode::BypassPermissions);
    }

    #[test]
    fn named_agent_switch_replaces_completion_requirement() {
        use super::apply_runtime_policies;
        use crate::config::{AgentDefinition, CompletionRequirement, PermissionMode};

        let mut src = AgentDefinition::browser_use();
        src.completion_requirement = Some(CompletionRequirement {
            tool: "browser_done".into(),
            reminder: "Mark the browse complete.".into(),
            recovery: None,
        });
        let mut dst = AgentDefinition::default_grok_build();
        dst.completion_requirement = Some(CompletionRequirement {
            tool: "complete_task".into(),
            reminder: "stale".into(),
            recovery: None,
        });
        apply_runtime_policies(&mut dst, &src);
        assert_eq!(
            dst.completion_requirement.as_ref().map(|c| c.tool.as_str()),
            Some("browser_done")
        );
        assert_eq!(dst.name, src.name);
        assert_eq!(dst.permission_mode, src.permission_mode);
        assert_ne!(dst.permission_mode, PermissionMode::Plan);
    }
}
