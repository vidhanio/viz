//! Pure timeline derivations: tool action classification, activity burst
//! folding, and path formatting.

use super::{
    AgentStanding, Item, Model, SubagentCall, Tool, ToolStatus, delegated_task, is_command,
    spawned_subagent, subagent_call,
};

/// Semantic category of a tool call for verbs, icons, and folded summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAction {
    /// Shell command execution.
    Command,
    /// File read.
    Read,
    /// File creation / write.
    Write,
    /// File edit / patch.
    Edit,
    /// File deletion.
    Delete,
    /// File move / rename.
    Move,
    /// Search / grep / glob.
    Search,
    /// Web fetch.
    Fetch,
    /// Any other tool.
    Other,
}

impl ToolAction {
    /// Summary order: mutations first, then reads and searches.
    pub const ALL: [Self; 9] = [
        Self::Edit,
        Self::Write,
        Self::Delete,
        Self::Move,
        Self::Command,
        Self::Read,
        Self::Search,
        Self::Fetch,
        Self::Other,
    ];
    /// Total number of [`ToolAction`] variants.
    pub const COUNT: usize = Self::ALL.len();

    /// Classifies `tool` by its normalized name.
    #[must_use]
    pub fn of(tool: &Tool) -> Self {
        match tool.name.as_str() {
            name if is_command(name) => Self::Command,
            "read" | "read_file" => Self::Read,
            "write" | "create" | "write_file" => Self::Write,
            "edit" | "apply_patch" | "edit_file" => Self::Edit,
            "delete" => Self::Delete,
            "move" => Self::Move,
            "search" | "grep" | "glob" | "web_search" => Self::Search,
            "fetch" | "web_fetch" => Self::Fetch,
            _ => Self::Other,
        }
    }

    /// Position in [`Self::ALL`].
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&a| a == self)
            .unwrap_or_default()
    }

    /// Whether the tool's summary names a filesystem path.
    #[must_use]
    pub const fn on_file(self) -> bool {
        matches!(
            self,
            Self::Read | Self::Write | Self::Edit | Self::Delete | Self::Move
        )
    }

    /// Action verb (past or present progressive), or `None` for unknown tools.
    #[must_use]
    pub const fn verb(self, done: bool) -> Option<&'static str> {
        Some(match (self, done) {
            (Self::Command, true) => "Ran",
            (Self::Command, false) => "Running",
            (Self::Read, true) => "Read",
            (Self::Read, false) => "Reading",
            (Self::Write, true) => "Wrote",
            (Self::Write, false) => "Writing",
            (Self::Edit, true) => "Edited",
            (Self::Edit, false) => "Editing",
            (Self::Delete, true) => "Deleted",
            (Self::Delete, false) => "Deleting",
            (Self::Move, true) => "Moved",
            (Self::Move, false) => "Moving",
            (Self::Search, true) => "Searched",
            (Self::Search, false) => "Searching",
            (Self::Fetch, true) => "Fetched",
            (Self::Fetch, false) => "Fetching",
            (Self::Other, _) => return None,
        })
    }

    /// Summary clause for `n` calls of this action kind.
    #[must_use]
    pub fn clause(self, n: usize) -> String {
        let count = |one: &'static str, many: &'static str| format!("{n} {}", plural(n, one, many));
        match self {
            Self::Command => format!("ran {}", count("command", "commands")),
            Self::Read => format!("read {}", count("file", "files")),
            Self::Write => format!("wrote {}", count("file", "files")),
            Self::Edit => format!("edited {}", count("file", "files")),
            Self::Delete => format!("deleted {}", count("file", "files")),
            Self::Move => format!("moved {}", count("file", "files")),
            Self::Search if n == 1 => "searched".to_owned(),
            Self::Search => format!("searched {n} times"),
            Self::Fetch => format!("fetched {}", count("page", "pages")),
            Self::Other => format!("used {}", count("tool", "tools")),
        }
    }
}

/// Which mark represents an activity burst header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityMark {
    /// Reasoning icon.
    Brain,
    /// Delegated subagent icon.
    Agent,
    /// Icon for a tool action category.
    Action(ToolAction),
}

/// Summary tally of a contiguous activity burst of reasoning and tool calls.
#[derive(Debug)]
pub struct ActivityTally<'a> {
    /// Count of reasoning entries.
    pub thoughts: usize,
    /// Whether any reasoning entry is still streaming.
    pub thinking: bool,
    /// Settled tool counts per [`ToolAction`].
    pub actions: [usize; ToolAction::COUNT],
    /// Count of delegated task spawns.
    pub spawns: usize,
    /// Counts of messages, status checks, and stops on delegated tasks.
    pub calls: [usize; 3],
    /// Subagent standings across spawned tasks.
    pub agents: [usize; AgentStanding::COUNT],
    /// Count of failed tool calls.
    pub failures: usize,
    /// Most recent non-empty thought text for folded preview.
    pub last_thought: Option<&'a str>,
    /// Currently running or pending tool, kept visible while folded.
    pub active_tool: Option<&'a Tool>,
}

impl<'a> ActivityTally<'a> {
    /// Folds `burst` into a summary tally.
    #[must_use]
    pub fn of(model: &Model, burst: impl Iterator<Item = (&'a Item, bool)>) -> Self {
        let mut tally = Self {
            thoughts: 0,
            thinking: false,
            actions: [0; ToolAction::COUNT],
            spawns: 0,
            calls: [0; 3],
            agents: [0; AgentStanding::COUNT],
            failures: 0,
            last_thought: None,
            active_tool: None,
        };
        for (item, streaming) in burst {
            match item {
                Item::Thinking { text, .. } => {
                    tally.thoughts += 1;
                    tally.thinking |= streaming;
                    let text = text.trim();
                    if !text.is_empty() {
                        tally.last_thought = Some(text);
                    }
                }
                Item::Tool(tool) if spawned_subagent(tool) => {
                    tally.spawns += 1;
                    tally.agents
                        [AgentStanding::of(model, tool, delegated_task(model, tool)).index()] += 1;
                }
                Item::Tool(tool)
                    if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running) =>
                {
                    tally.active_tool = Some(tool);
                }
                Item::Tool(tool) => {
                    match subagent_call(tool) {
                        Some(SubagentCall::Message) => tally.calls[0] += 1,
                        Some(SubagentCall::Read | SubagentCall::List) => tally.calls[1] += 1,
                        Some(SubagentCall::Cancel) => tally.calls[2] += 1,
                        _ => tally.actions[ToolAction::of(tool).index()] += 1,
                    }
                    tally.failures += usize::from(tool.status == ToolStatus::Failed);
                }
                _ => {}
            }
        }
        tally
    }

    /// Total settled tool calls in this burst.
    #[must_use]
    pub fn tools(&self) -> usize {
        self.actions.iter().chain(&self.calls).sum()
    }

    /// Whether the burst contains only reasoning and no tool calls or spawns.
    #[must_use]
    pub fn only_thought(&self) -> bool {
        self.tools() == 0 && self.spawns == 0 && self.active_tool.is_none()
    }

    /// Human-readable heading for the folded activity burst.
    #[must_use]
    pub fn label(&self) -> String {
        let mut clauses = Vec::new();
        if self.thinking {
            clauses.push("thinking".to_owned());
        } else if self.thoughts > 0 {
            clauses.push("thought".to_owned());
        }
        for action in ToolAction::ALL {
            let n = self.actions[action.index()];
            if n > 0 {
                clauses.push(action.clause(n));
            }
        }
        if self.spawns > 0 {
            clauses.push(format!(
                "delegated {} {}",
                self.spawns,
                plural(self.spawns, "task", "tasks")
            ));
        }
        let [messages, checks, stops] = self.calls;
        if messages > 0 {
            clauses.push(format!(
                "messaged {messages} {}",
                plural(messages, "task", "tasks")
            ));
        }
        if checks > 0 {
            clauses.push("checked tasks".to_owned());
        }
        if stops > 0 {
            clauses.push(format!(
                "stopped {stops} {}",
                plural(stops, "task", "tasks")
            ));
        }
        let mut label = match clauses.as_slice() {
            [] => "working".to_owned(),
            [only] => only.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        };
        if let Some(first) = label.get(..1) {
            let upper = first.to_uppercase();
            label.replace_range(..1, &upper);
        }
        label
    }

    /// Primary icon category representing this activity burst.
    #[must_use]
    pub fn mark(&self) -> ActivityMark {
        if self.only_thought() {
            return ActivityMark::Brain;
        }
        if self.spawns > 0 || self.calls.iter().sum::<usize>() > 0 {
            return ActivityMark::Agent;
        }
        ToolAction::ALL
            .into_iter()
            .find(|action| self.actions[action.index()] > 0)
            .map_or_else(
                || {
                    if self.thoughts > 0 {
                        ActivityMark::Brain
                    } else {
                        ActivityMark::Action(
                            self.active_tool.map_or(ToolAction::Other, ToolAction::of),
                        )
                    }
                },
                ActivityMark::Action,
            )
    }
}

/// Whether a timeline item belongs in a collapsible activity burst.
#[must_use]
pub const fn is_activity(item: &Item) -> bool {
    matches!(item, Item::Tool(_) | Item::Thinking { .. })
}

/// Converts an unknown MCP or harness tool identifier into readable words.
#[must_use]
pub fn humanize_tool(name: &str) -> String {
    let name = name.rsplit("__").next().unwrap_or(name);
    let mut words = name
        .split(['_', '-', ' ', '.'])
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(first) = words.get(..1) {
        let upper = first.to_uppercase();
        words.replace_range(..1, &upper);
    }
    words
}

/// Picks singular or plural noun form for `n`.
#[must_use]
pub const fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

/// Splits `path` into its directory prefix (with trailing slash) and filename.
#[must_use]
pub fn split_path(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rfind('/')
        .map_or(("", trimmed), |cut| (&trimmed[..=cut], &trimmed[cut + 1..]))
}

/// Strips the open thread's working directory prefix from `path` when inside
/// it.
#[must_use]
pub fn relative_path<'a>(model: &Model, path: &'a str) -> &'a str {
    model
        .thread_state()
        .and_then(|state| path.strip_prefix(state.cwd.trim_end_matches('/')))
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path)
}

/// First non-empty trimmed line of `text`.
#[must_use]
pub fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}
