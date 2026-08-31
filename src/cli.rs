//! The clap derive tree, and **nothing else**. No handler, no type used elsewhere.
//!
//! Rule 4 of the parallel build: **no agent adds a CLI arg.** The full tree ships in wave
//! 0 straight off DESIGN.md's CLI reference; v0.2 subcommands ship
//! `#[command(hide = true)]` so v0.2 fills bodies and never edits this file. A missing
//! flag is a request to **F**, batched between waves.
//!
//! Free-text reasons (`--why`, `--note`, titles) all carry `allow_hyphen_values` so
//! `--why "-- not shipped"` parses instead of becoming an unknown flag.
//!
//! Owner: **F** (foundation). FROZEN.

use std::path::PathBuf;
use std::sync::OnceLock;

use clap::{Args, Parser, Subcommand, ValueEnum};

// ─────────────────────────────────────────────────────────────────────────────
// Root
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "kanspec",
    version,
    about = "kanban + spec review over plain git-tracked files",
    long_about = "kanspec turns a .kanspec/ directory of plain markdown files — tickets, \
                  proposals, living specs, decisions, quirks — into a live kanban board and \
                  review surface for coding agents.\n\n\
                  Merge state, staleness and stuck-ness are COMPUTED from git, never asserted: \
                  no command, UI action or file field can claim a ticket landed.",
    after_help = "Every command takes --json. Typed errors name the exact next command.\n\
                  Long-form workflow docs ship in the binary: `kanspec instructions`.",
    arg_required_else_help = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    // `help_heading` keeps the three globals in their own block at the foot of EVERY
    // subcommand's help, instead of interleaved with that command's own flags by display
    // order — which is what makes `kanspec done --help` readable.
    /// Emit machine-readable JSON on stdout instead of the human rendering
    #[arg(long, global = true, help_heading = "Global options")]
    pub json: bool,

    /// When to colorize output
    #[arg(
        long,
        global = true,
        value_enum,
        default_value_t = ColorChoice::Auto,
        value_name = "WHEN",
        help_heading = "Global options"
    )]
    pub color: ColorChoice,

    /// Operate on this repository instead of the current directory
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help_heading = "Global options"
    )]
    pub repo: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// `"kanspec"` or `"ks"` — set once by `run()`, because the log note and every fix
    /// line should name the binary the user actually typed.
    pub fn invoked_as(&self) -> &'static str {
        invoked_as()
    }
}

static INVOKED_AS: OnceLock<&'static str> = OnceLock::new();

pub fn set_invoked_as(name: &'static str) {
    let _ = INVOKED_AS.set(name);
}

pub fn invoked_as() -> &'static str {
    INVOKED_AS.get().copied().unwrap_or("kanspec")
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

// ─────────────────────────────────────────────────────────────────────────────
// The command tree — declaration order IS `--help` order, grouped like DESIGN.md
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Subcommand, Debug)]
pub enum Command {
    // ── SETUP ────────────────────────────────────────────────────────────────
    /// Scaffold .kanspec/, .gitattributes (merge=union), git hooks, gitignore cache/
    Init(InitArgs),
    /// Install the agent snippet + hooks for claude | cursor | codex
    Setup(SetupArgs),
    /// Prove the invariants: log-trail legality, dead globs, orphan deps, ledger
    Doctor(DoctorArgs),
    /// Print the workflow docs that ship inside the binary
    Instructions(InstructionsArgs),
    /// Print a shell completion script
    Completions(CompletionsArgs),

    // ── TICKETS ──────────────────────────────────────────────────────────────
    /// Create a ticket
    New(NewArgs),
    /// The claimable queue: todo with every dep satisfied (derived)
    Ready(ReadyArgs),
    /// Atomically claim a ticket -> doing; creates branch ks/<id>-slug (+ worktree)
    Start(StartArgs),
    /// doing -> review; records the head SHA from git and the PR number
    Ship(ShipArgs),
    /// The close-out gate: requires a git-detected merge
    Done(DoneArgs),
    /// doing -> todo; an explicit unclaim so nothing rots silently
    Park(ParkArgs),
    /// any -> dropped
    Drop(DropArgs),
    /// Show one ticket with its badges, context and owed verb
    Show(ShowArgs),
    /// Print a ticket's transition log
    Log(LogArgs),
    /// Which ticket owns this branch / worktree
    Where(WhereArgs),
    /// List tickets
    Ls(LsArgs),
    /// Record an attested state reset for a ticket whose log cannot replay
    Repair(RepairArgs),

    // ── STATUS & GIT TRUTH ───────────────────────────────────────────────────
    /// THE anti-stuck query: everything non-terminal grouped by who owes the next verb
    Status(StatusArgs),
    /// Merge detection from git/gh; results go to the cache only
    Scan(ScanArgs),
    /// Terminal board; --export writes a markdown snapshot for a PR
    Board(BoardArgs),
    /// Serve the live board + review pages (foreground, loopback, SSE)
    Up(UpArgs),
    /// Open the board in a browser
    Open(OpenArgs),
    /// Per-ticket CI state via homerunner (local) or gh
    #[command(hide = true)]
    Ci(CiArgs),

    // ── PROVENANCE & DECISIONS ───────────────────────────────────────────────
    /// Every standing rule steering agents now, with provenance
    Rules(RulesArgs),
    /// Walk the chain: rule -> proposal item -> tickets -> PR
    Why(WhyArgs),
    /// Mint a PROPOSED decision (only a human can accept it)
    Decide(DecideArgs),
    /// Accept a proposed decision — human only; freezes the body
    Accept(AcceptArgs),
    /// Supersede an accepted decision with a new one
    Supersede(SupersedeArgs),
    /// Revoke an accepted decision — the human's kill switch
    Revoke(RevokeArgs),

    // ── KNOWLEDGE ────────────────────────────────────────────────────────────
    /// Render the feature map; --stale lists tripwired specs
    Features(FeaturesArgs),
    /// Living per-capability specs
    Spec(SpecArgs),
    /// Capture or retire a landmine
    Quirk(QuirkArgs),
    /// List quirks, path-scoped
    Quirks(QuirksArgs),
    /// The agent working set, injected by hooks (~1.5k tokens)
    Prime(PrimeArgs),

    // ── PROPOSALS & REVIEW (v0.2) ────────────────────────────────────────────
    /// Scaffold a one-page proposal
    #[command(hide = true)]
    Propose(ProposeArgs),
    /// draft -> review; prints the review page URL
    #[command(hide = true)]
    Review(ReviewArgs),
    /// Read review threads
    #[command(hide = true)]
    Comments(CommentsArgs),
    /// Add, reply to, or resolve a review thread
    #[command(hide = true)]
    Comment(CommentArgs),
    /// Approve a proposal — refuses while threads are unresolved
    #[command(hide = true)]
    Approve(ApproveArgs),
    /// THE close gate: refuses until every [cN]/[pN] item is dispositioned
    #[command(hide = true)]
    Close(CloseArgs),
    /// Abandon a proposal
    #[command(hide = true)]
    Abandon(AbandonArgs),
    /// Promote a proposal prescription into a standing record
    #[command(hide = true)]
    Promote(PromoteArgs),
    /// Expire a proposal prescription
    #[command(hide = true)]
    Expire(ExpireArgs),
    /// The Stop hook: exit 2 while the tracker disagrees with the working tree
    #[command(hide = true)]
    Landcheck(LandcheckArgs),
}

// ─────────────────────────────────────────────────────────────────────────────
// SETUP
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Reinstall the git hooks over an existing store, preserving foreign hooks
    #[arg(long)]
    pub refresh_hooks: bool,
}

#[derive(Args, Debug)]
pub struct SetupArgs {
    /// Which agent to wire up
    #[arg(value_enum)]
    pub agent: Agent,
    /// Uninstall symmetrically — restores any hook kanspec displaced
    #[arg(long)]
    pub remove: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Cursor,
    Codex,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Apply every finding that has a mechanical repair
    #[arg(long)]
    pub fix: bool,
}

#[derive(Args, Debug)]
pub struct InstructionsArgs {
    /// start | done | review | close | config — omit to list every topic
    #[arg(value_name = "TOPIC")]
    pub topic: Option<String>,
}

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    /// The shell to generate a completion script for
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

// ─────────────────────────────────────────────────────────────────────────────
// TICKETS
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct NewArgs {
    /// What the work is, in one line
    #[arg(value_name = "TITLE", allow_hyphen_values = true)]
    pub title: String,
    /// The capability this belongs to
    #[arg(long, value_name = "SPEC")]
    pub spec: Option<String>,
    /// The proposal this implements
    #[arg(long, value_name = "PROPOSAL")]
    pub proposal: Option<String>,
    /// Blocks until this ticket is satisfied; repeatable
    #[arg(long = "dep", value_name = "TICKET")]
    pub deps: Vec<String>,
    /// Leftover scope from a ticket's own steps
    #[arg(long, value_name = "TICKET")]
    pub followup_of: Option<String>,
    /// Stamp discovered_in explicitly instead of using the claimed ticket
    #[arg(long, value_name = "TICKET", conflicts_with = "no_link")]
    pub from: Option<String>,
    /// Do not stamp discovered_in at all
    #[arg(long)]
    pub no_link: bool,
}

#[derive(Args, Debug)]
pub struct ReadyArgs {
    /// Show only tickets for this capability
    #[arg(long, value_name = "SPEC")]
    pub spec: Option<String>,
    /// How many to print
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub struct StartArgs {
    /// The ticket to claim
    pub id: String,
    /// Also create a linked worktree under [worktree_dir]
    #[arg(long)]
    pub worktree: bool,
}

#[derive(Args, Debug)]
pub struct ShipArgs {
    /// The ticket whose work is ready for review
    pub id: String,
    /// The pull request number, recorded for merge-detection rung 2
    #[arg(long, value_name = "N")]
    pub pr: Option<u64>,
}

#[derive(Args, Debug)]
pub struct DoneArgs {
    /// The ticket to close out
    pub id: String,

    // ── leftover triage: every unchecked step needs one of these three ────────
    /// Spawn a followup ticket for an unchecked step; repeatable
    #[arg(long, value_name = "TITLE", allow_hyphen_values = true)]
    pub spawn: Vec<String>,
    /// Drop an unchecked step with a reason: `--drop-step "3:superseded"`; repeatable
    #[arg(long, value_name = "N:REASON", allow_hyphen_values = true)]
    pub drop_step: Vec<String>,
    /// Mark an unchecked step as actually done; repeatable
    #[arg(long, value_name = "N")]
    pub actually_done: Vec<usize>,
    /// Record that there is no leftover work — required when nothing is spawned
    #[arg(long, conflicts_with = "spawn")]
    pub no_followups: bool,

    // ── knowledge checkpoint ──────────────────────────────────────────────────
    /// Record why the branch touched a spec's globs without editing the spec
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub spec_unchanged: Option<String>,
    /// Capture a quirk learned on this ticket; repeatable
    #[arg(long, value_name = "TITLE", allow_hyphen_values = true)]
    pub quirk: Vec<String>,
    /// Paths for the quirk being captured; repeatable
    #[arg(long, value_name = "GLOB")]
    pub quirk_paths: Vec<String>,
    /// Record that no quirks were learned
    #[arg(long, conflicts_with = "quirk")]
    pub no_quirks: bool,
    /// Mint a PROPOSED decision made during this ticket; repeatable
    #[arg(long, value_name = "TITLE", allow_hyphen_values = true)]
    pub decision: Vec<String>,
    /// Record that no decisions were made
    #[arg(long, conflicts_with = "decision")]
    pub no_decisions: bool,

    // ── the merge gate's recorded escape ──────────────────────────────────────
    /// Chore/docs escape: close without a git-detected merge. Requires --why
    #[arg(long, requires = "why")]
    pub no_code: bool,
    /// The recorded reason for --no-code
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub why: Option<String>,
}

#[derive(Args, Debug)]
pub struct ParkArgs {
    /// The ticket to unclaim
    pub id: String,
    /// Why the work is being put down — recorded on the ticket
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub why: String,
}

#[derive(Args, Debug)]
pub struct DropArgs {
    /// The ticket that will never be done
    pub id: String,
    /// Why the ticket will never be done — recorded on the ticket
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub why: String,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// The ticket to show
    pub id: String,
}

#[derive(Args, Debug)]
pub struct LogArgs {
    /// The ticket whose transition log to print
    pub id: String,
}

#[derive(Args, Debug)]
pub struct WhereArgs {
    /// Resolve this branch instead of the one checked out here
    #[arg(long, value_name = "BRANCH")]
    pub branch: Option<String>,
}

#[derive(Args, Debug)]
pub struct LsArgs {
    /// Only this capability
    #[arg(long, value_name = "SPEC")]
    pub spec: Option<String>,
    /// Only tickets claimed by me
    #[arg(long)]
    pub mine: bool,
    /// Only tickets past the stall window
    #[arg(long)]
    pub stalled: bool,
    /// Only tickets whose work is not detected on main
    #[arg(long)]
    pub unmerged: bool,
    /// Include done and dropped tickets
    #[arg(long, conflicts_with_all = ["mine", "stalled", "unmerged"])]
    pub all: bool,
}

#[derive(Args, Debug)]
pub struct RepairArgs {
    /// The ticket whose ## Log cannot replay
    pub id: String,
    /// The human attestation: why the log cannot replay and what the state really is
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub why: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// STATUS & GIT TRUTH
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Only one owner group
    #[arg(long, value_enum, value_name = "WHO")]
    pub owner: Option<OwnerFilter>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum OwnerFilter {
    You,
    Agent,
    Watching,
}

#[derive(Args, Debug)]
pub struct ScanArgs {
    /// Scan only this ticket
    #[arg(value_name = "ID")]
    pub id: Option<String>,
    /// Print the ladder's reasoning, rung by rung
    #[arg(long)]
    pub explain: bool,
    /// The recorded human override for a genuinely ambiguous merge. Requires --why
    #[arg(long, value_name = "ID", requires = "why")]
    pub confirm: Option<String>,
    /// The attestation recorded on the ticket's ## Log alongside --confirm
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub why: Option<String>,
    /// Print nothing on success — for the post-merge / post-checkout git hooks
    #[arg(long)]
    pub quiet: bool,
    /// Skip `git fetch origin`; results are stamped with the fetch age instead
    #[arg(long)]
    pub no_fetch: bool,
}

#[derive(Args, Debug)]
pub struct BoardArgs {
    /// Write a markdown snapshot instead of printing the terminal board
    #[arg(long, value_name = "FILE")]
    pub export: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct UpArgs {
    /// Bind 127.0.0.1:<PORT>
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,
    /// Open a browser once the server is listening
    #[arg(long)]
    pub open: bool,
}

#[derive(Args, Debug)]
pub struct OpenArgs {
    /// A ticket, proposal or decision id — omit for the board
    #[arg(value_name = "ID")]
    pub id: Option<String>,
}

#[derive(Args, Debug)]
pub struct CiArgs {
    #[command(subcommand)]
    pub cmd: Option<CiCommand>,
}

#[derive(Subcommand, Debug)]
pub enum CiCommand {
    /// Print the failure digest for a ticket's latest failed job
    Why {
        /// The ticket whose latest failed job to digest
        id: String,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// PROVENANCE & DECISIONS
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct RulesArgs {
    /// Scope to the rules that steer edits to this path; repeatable
    #[arg(long = "path", value_name = "FILE")]
    pub paths: Vec<String>,
    /// Warn about rules whose provenance or relevance has decayed
    #[arg(long)]
    pub audit: bool,
    /// Adopt pre-kanspec spec rules that carry no provenance token
    #[arg(long)]
    pub adopt: bool,
}

#[derive(Args, Debug)]
pub struct WhyArgs {
    /// A rule anchor (auth#lockout), an item anchor (p-7de2#c3), or a ticket id
    #[arg(value_name = "ANCHOR")]
    pub anchor: String,
}

#[derive(Args, Debug)]
pub struct DecideArgs {
    /// The decision, stated as a claim
    #[arg(value_name = "TITLE", allow_hyphen_values = true)]
    pub title: String,
    /// Provenance: the proposal item or ticket this decision came out of
    #[arg(long, value_name = "SOURCE")]
    pub from: Option<String>,
    /// Where it steers agents; repeatable
    #[arg(long, value_name = "GLOB")]
    pub scope: Vec<String>,
}

#[derive(Args, Debug)]
pub struct AcceptArgs {
    /// The proposed decision to accept
    pub id: String,
}

#[derive(Args, Debug)]
pub struct SupersedeArgs {
    /// The accepted decision being replaced
    pub id: String,
    /// The title of the decision that replaces it
    #[arg(long = "with", value_name = "TITLE", allow_hyphen_values = true)]
    pub with: String,
}

#[derive(Args, Debug)]
pub struct RevokeArgs {
    /// The accepted decision to revoke
    pub id: String,
    /// Why it no longer holds — recorded on the decision
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub why: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// KNOWLEDGE
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct FeaturesArgs {
    /// List the specs the staleness tripwire has fired on
    #[arg(long)]
    pub stale: bool,
    /// Record "no behaviour change" for a stale spec. Requires --why
    #[arg(long, value_name = "SPEC", requires = "why")]
    pub confirm: Option<String>,
    /// The attestation stored in the spec's frontmatter alongside --confirm
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub why: Option<String>,
}

#[derive(Args, Debug)]
pub struct SpecArgs {
    #[command(subcommand)]
    pub cmd: SpecCommand,
}

#[derive(Subcommand, Debug)]
pub enum SpecCommand {
    /// Scaffold a living spec with its feature one-liner and code globs
    New {
        /// The capability name — the spec's file stem
        name: String,
        /// The one-liner that feeds the generated feature map
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        feature: Option<String>,
        /// A glob the staleness tripwire watches; repeatable
        #[arg(long = "code", value_name = "GLOB")]
        code: Vec<String>,
    },
    /// Print a spec with its rules and provenance tokens
    Show {
        /// The capability to print
        name: String,
    },
    /// Search rule text across every spec
    Grep {
        /// Text to search for across every spec's rule bullets
        #[arg(value_name = "PATTERN", allow_hyphen_values = true)]
        pattern: String,
    },
}

#[derive(Args, Debug)]
pub struct QuirkArgs {
    #[command(subcommand)]
    pub cmd: QuirkCommand,
}

#[derive(Subcommand, Debug)]
pub enum QuirkCommand {
    /// Capture a landmine at the moment of pain
    Add {
        /// The landmine, in one line
        #[arg(value_name = "TITLE", allow_hyphen_values = true)]
        title: String,
        /// Where it bites; repeatable
        #[arg(long = "paths", value_name = "GLOB", required = true)]
        paths: Vec<String>,
        /// How badly it bites
        #[arg(long = "sev", value_enum, default_value_t = SeverityArg::Gotcha)]
        severity: SeverityArg,
        /// The ticket where this was learned
        #[arg(long, value_name = "TICKET")]
        from: Option<String>,
    },
    /// Retire a quirk by evidence — the ticket that actually fixed it
    Fix {
        /// The quirk that is now genuinely fixed
        id: String,
        /// The ticket that actually fixed it — retired only by evidence
        #[arg(long = "by", value_name = "TICKET")]
        by: String,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum SeverityArg {
    Landmine,
    Gotcha,
    Debt,
}

#[derive(Args, Debug)]
pub struct QuirksArgs {
    /// Only quirks matching these globs; repeatable
    #[arg(long = "paths", value_name = "GLOB")]
    pub paths: Vec<String>,
    /// PostToolUse hook form: warn if this file matches an active quirk
    #[arg(long, value_name = "FILE")]
    pub touch: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PrimeArgs {
    /// Scope the standing-rules section to these paths; repeatable
    #[arg(long = "path", value_name = "FILE")]
    pub paths: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// PROPOSALS & REVIEW (v0.2)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct ProposeArgs {
    /// What the proposal is about, in one line
    #[arg(value_name = "TITLE", allow_hyphen_values = true)]
    pub title: String,
    /// A capability this proposal changes; repeatable
    #[arg(long, value_name = "SPEC")]
    pub spec: Vec<String>,
}

#[derive(Args, Debug)]
pub struct ReviewArgs {
    /// The proposal to put up for review
    pub id: String,
}

#[derive(Args, Debug)]
pub struct CommentsArgs {
    /// A proposal or ticket id — omit for everything with open threads
    #[arg(value_name = "ID")]
    pub id: Option<String>,
    /// Only threads nobody has resolved
    #[arg(long)]
    pub unresolved: bool,
}

#[derive(Args, Debug)]
pub struct CommentArgs {
    #[command(subcommand)]
    pub cmd: CommentCommand,
}

#[derive(Subcommand, Debug)]
pub enum CommentCommand {
    /// Start a thread on a visible item anchor
    Add {
        /// The visible item anchor, e.g. p-7de2#c3
        #[arg(value_name = "TARGET")]
        target: String,
        /// The comment body
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: String,
    },
    /// Reply in an existing thread
    Reply {
        /// The thread to reply in
        #[arg(value_name = "CM-ID")]
        id: String,
        /// The reply body
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        body: String,
    },
    /// Resolve a thread — the note is what changed, and it is required
    Resolve {
        /// The thread to resolve
        #[arg(value_name = "CM-ID")]
        id: String,
        /// What changed — required, because that is the whole point
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        note: String,
    },
}

#[derive(Args, Debug)]
pub struct ApproveArgs {
    /// The proposal to approve
    pub id: String,
}

#[derive(Args, Debug)]
pub struct CloseArgs {
    /// The proposal to close
    pub id: String,
    /// Spawn a ticket for this item; repeatable
    #[arg(long, value_name = "ITEM")]
    pub followup: Vec<String>,
    /// Drop this item; repeatable. Requires --note
    #[arg(long, value_name = "ITEM", requires = "note")]
    pub dropped: Vec<String>,
    /// The recorded reason accompanying --dropped
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
    pub note: Option<String>,
}

#[derive(Args, Debug)]
pub struct AbandonArgs {
    /// The proposal to abandon
    pub id: String,
    /// Why it is being abandoned — recorded on the proposal
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub why: String,
}

#[derive(Args, Debug)]
pub struct PromoteArgs {
    /// The prescription anchor, e.g. p-7de2#p1
    #[arg(value_name = "ITEM")]
    pub item: String,
    /// Which kind of standing record to mint
    #[arg(long = "as", value_enum, value_name = "KIND")]
    pub as_kind: PromoteKind,
    /// Where the standing record steers agents; repeatable
    #[arg(long, value_name = "GLOB")]
    pub scope: Vec<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum PromoteKind {
    Decision,
    Spec,
    Quirk,
}

#[derive(Args, Debug)]
pub struct ExpireArgs {
    /// The prescription anchor, e.g. p-7de2#p2
    #[arg(value_name = "ITEM")]
    pub item: String,
    /// Why it no longer applies — recorded in the ledger
    #[arg(long, value_name = "REASON", allow_hyphen_values = true)]
    pub reason: String,
}

#[derive(Args, Debug)]
pub struct LandcheckArgs {
    /// Report what would block without exiting 2
    #[arg(long)]
    pub dry_run: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_tree_is_well_formed() {
        // `global + required` is a DEBUG-ONLY assert that is compiled out of release, so
        // this is not optional.
        Cli::command().debug_assert();
    }

    #[test]
    fn the_json_flag_is_global() {
        let c = Cli::try_parse_from(["kanspec", "show", "t-9c41", "--json"]).unwrap();
        assert!(c.json);
    }

    #[test]
    fn free_text_reasons_accept_leading_hyphens() {
        let c = Cli::try_parse_from(["kanspec", "park", "t-9c41", "--why", "-- blocked"]).unwrap();
        match c.command {
            Command::Park(a) => assert_eq!(a.why, "-- blocked"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_code_requires_a_recorded_reason() {
        assert!(Cli::try_parse_from(["kanspec", "done", "t-9c41", "--no-code"]).is_err());
        assert!(
            Cli::try_parse_from(["kanspec", "done", "t-9c41", "--no-code", "--why", "docs"])
                .is_ok()
        );
    }

    #[test]
    fn spawn_and_no_followups_are_mutually_exclusive() {
        assert!(Cli::try_parse_from([
            "kanspec",
            "done",
            "t-9c41",
            "--spawn",
            "x",
            "--no-followups"
        ])
        .is_err());
    }

    #[test]
    fn confirm_requires_its_attestation() {
        assert!(Cli::try_parse_from(["kanspec", "scan", "--confirm", "t-9c41"]).is_err());
        assert!(
            Cli::try_parse_from(["kanspec", "scan", "--confirm", "t-9c41", "--why", "squash"])
                .is_ok()
        );
    }

    #[test]
    fn bare_invocation_is_a_usage_error_not_success() {
        let e = Cli::try_parse_from(["kanspec"]).unwrap_err();
        assert_ne!(
            e.kind(),
            clap::error::ErrorKind::DisplayHelp,
            "D-16: bare kanspec exits 64"
        );
    }

    #[test]
    fn v02_subcommands_parse_even_though_they_are_hidden() {
        assert!(Cli::try_parse_from(["kanspec", "propose", "Login rate limiting"]).is_ok());
        assert!(Cli::try_parse_from([
            "kanspec",
            "comment",
            "resolve",
            "cm-88f1",
            "--note",
            "c3 -> p-8a10"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "kanspec",
            "promote",
            "p-7de2#p1",
            "--as",
            "decision",
            "--scope",
            "src/auth/**"
        ])
        .is_ok());
    }
}
