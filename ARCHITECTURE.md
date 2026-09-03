# kanspec — implementation contract v1

**Architecture: Skeleton-First, sealed at the seams.**
Base = *Skeleton-First* (2 of 3 judges). Grafted: *One Gate*'s pure planner / `Plan` / in-lock
snapshot / `closed_ids` / doctor registry; *Sealed Keel*'s closed key enums, `Sha`/`HeadSha`,
`ScanToken`, `NoCodeWaiver`, `Fixes(Fix, Vec<Fix>)`, `Verb::Confirm`, private `KanspecDir`, rich
`Unknown`. Every fatal flaw the judges named is closed below and marked ✅.

> **Thesis.** kanspec is a file-munger with a git oracle bolted on. One crate, two bins, flat
> `src/*.rs`, no workspace, no trait with one implementation, no async outside `up`. Three things
> carry the whole design: **`Ctx`** is built once in `run()` before dispatch, so worktree
> unification and actor/clock injection touch every command without any command knowing they
> exist; **`Store::transact`** is the only code that writes a byte under `.kanspec/`, and it takes
> the flock, reloads a fresh `Snapshot` *inside* the lock, runs a **pure planner**
> `fn(&Snapshot, &Facts, &Args, &Minter) -> Result<Plan>`, validates, applies, and replays each
> touched ticket's log before releasing; **`Render: Serialize`** makes `--json` the Serialize impl
> so the two surfaces cannot drift. Compile-time guarantees are spent in exactly four places where
> they buy a real safety property — `MergedProof`, `TicketKey`, `HumanActor`, `KanspecDir` — and
> nowhere else.

**Everything in §2 was compiled and tested before this document was written**
(`/private/tmp/claude-501/.../scratchpad/arch-verify`, 6 tests green, clippy clean, incl.
`Arc<Ctx>: Send + Sync`, flock, the seals, `plan_ship`/`plan_done`, the transition table, and
`replay`). `E0742` was reproduced to prove why the seals are colocated (§2.7).

---

## 1. Module tree

Flat `src/*.rs` (house style: `~/dev/homerunner`). Every file has **exactly one owner** (§10).
Scope tag: **v0.1** = the weekend cut · **v0.2** = declared in wave 0 as `unimplemented!("v0.2")`
with a `#[command(hide = true)]` clap arm, so v0.2 fills bodies and never edits a frozen file.

```
kanspec/
├── Cargo.toml                 pinned deps, 2 [[bin]], profile.release lto+strip        F   v0.1
├── build.rs                   println!("cargo:rerun-if-changed=assets") — MANDATORY    F   v0.1
├── rust-toolchain.toml        pin stable; edition 2021                                 F   v0.1
├── assets/                    embedded SPA: index.html, app.js, style.css (vanilla)    S8  v0.1
├── docs/                      long-form workflow docs for `kanspec instructions`       S7  v0.1
└── src/
    ├── lib.rs                 run(invoked_as)->ExitCode; Ctx build; the whole dispatch  F  v0.1
    ├── bin/kanspec.rs         3 lines -> kanspec::run("kanspec")                        F  v0.1
    ├── bin/ks.rs              3 lines -> kanspec::run("ks")                             F  v0.1
    ├── cli.rs                 clap derive tree ONLY: Cli, Command, every *Args. FROZEN  F  v0.1
    ├── ctx.rs                 Ctx, Actor, HumanActor, OutMode. Send+Sync. FROZEN        F  v0.1
    ├── error.rs               KsError (8 shapes), Fix/Fixes, ExitCode, GateDetail       F  v0.1
    ├── out.rs                 trait Render: Serialize; emit(); Line/Table/Style prims   F  v0.1
    ├── paths.rs               Repo::discover ladder; KanspecDir (private); Layout       F  v0.1
    ├── config.rs              Config + every field #[serde(default)]; toml 1.1          F  v0.1
    ├── ids.rs                 id_kind! per-kind newtypes, Minter, ItemRef, RuleRef      F  v0.1
    ├── keys.rs                TicketKey/SpecKey/... closed enums + RESERVED_DERIVED     F  v0.1
    ├── logentry.rs            the `## Log` line grammar: parse/format, one place        F  v0.1
    ├── model.rs               entity structs + Snapshot. Types only, no IO. FROZEN      F  v0.1
    ├── transitions.rs         Verb, next(), require(), replay(), LogViolation. FROZEN   F  v0.1
    ├── plan.rs                Op, Plan, Plan::validate, EntityRef. FROZEN               F  v0.1
    │
    ├── fm.rs                  lossless split, key index, surgical set(), writable()     S1 v0.1
    ├── lock.rs                flock(2) advisory lock -> LockToken (write capability)   S1 v0.1
    ├── store.rs               load_snapshot(); Store::transact — THE ONLY WRITER        S1 v0.1
    │
    ├── git.rs                 `git -C <primary>` shell-out; Sha/HeadSha seal; Pathspec  S2 v0.1
    ├── gh.rs                  gh JSON; KANSPEC_GH_FIXTURES replay seam (the ONE seam)   S2 v0.1
    │
    ├── scan.rs                the 4-rung ladder; MergedProof/NoCodeWaiver/ScanToken     S3 v0.1
    ├── cache.rs               cache/gitstate.json DTOs; total, versioned load           S3 v0.1
    │
    ├── derive.rs              PURE fns over &Snapshot: the whole derived surface        S4 v0.1
    ├── doctor.rs              CHECKS registry, Finding, --fix appliers                  S4 v0.1
    │
    ├── triage.rs              Triage: the typed done-gate argument (flags AND prompts)  S5 v0.1
    │
    ├── rulesdoc.rs            THE standing-rules generator + THE renderer               S6 v0.1
    ├── project.rs             KANSPEC-FEATURES.md / KANSPEC-ARCHITECTURE.md writers     S6 v0.1
    │
    ├── hooks.rs               rev-parse --git-path hooks; the <hook>.d/ dispatcher      S7 v0.1
    ├── setup.rs               claude|cursor|codex snippet + agent hooks (+ --remove)    S7 v0.1
    ├── instructions.rs        embedded docs/ topic printer                              S7 v0.1
    ├── ci.rs                  provider detect; homerunner rusqlite + SSE; gh fallback   S7 v0.2
    │
    ├── board.rs               BoardModel: one struct for `board` AND /api/board         S8 v0.1
    ├── server.rs              axum 0.8: /api/*, /events SSE, embedded-asset fallback    S8 v0.1
    │
    └── cmd/
        ├── mod.rs             `pub mod` lines ONLY. FROZEN                              F  v0.1
        ├── init.rs            init [--refresh-hooks]                                    S7 v0.1
        ├── setup.rs           setup claude|cursor|codex [--remove] / instructions /
        │                      completions                                               S7 v0.1
        ├── ticket.rs          new / show / ls / log / where                             S5 v0.1
        ├── flow.rs            ready / start / ship / park / drop                        S5 v0.1
        ├── done.rs            THE close-out gate (interactive + --json)                 S5 v0.1
        ├── status.rs          YOU / AGENT / WATCHING                                    S4 v0.1
        ├── doctor.rs          doctor [--fix]                                            S4 v0.1
        ├── scan.rs            scan [--explain] [--confirm]                              S3 v0.1
        ├── repair.rs          repair <id> --why (the recorded log reset)                S3 v0.1
        ├── spec.rs            spec new|show|grep                                        S6 v0.1
        ├── quirk.rs           quirk add|fix / quirks [--touch]                          S6 v0.1
        ├── decision.rs        decide / accept / supersede / revoke / why                S6 v0.1
        ├── rules.rs           rules [--path] [--audit] [--adopt]                        S6 v0.1
        ├── prime.rs           prime                                                     S6 v0.1
        ├── features.rs        features [--stale] [--confirm <spec>]                     S6 v0.1
        ├── board.rs           board [--export]                                          S8 v0.1
        ├── up.rs              up [--port] / open — the ONLY async entry point           S8 v0.1
        ├── proposal.rs        propose/review/approve/close/abandon                      V2 v0.2
        ├── comment.rs         comments / comment add|reply|resolve / promote / expire   V2 v0.2
        └── landcheck.rs       the Stop hook — the ONLY minter of BlockToken (exit 2)    V2 v0.2

tests/
├── common/mod.rs              TestRepo harness (template-dir + fs::copy). FROZEN        F
├── common/merges.rs           the six real merge shapes from recon. FROZEN              F
├── fixtures/                  adversarial frontmatter corpus, recorded gh JSON, goldens F
├── fm_bytes.rs                byte-stability, 100-edit idempotence, CRLF, adversarial   S1
├── lock.rs                    20-process contention; kill -9 releases                   S1
├── single_write_path.rs       GREP: no fs writes outside store.rs (+2 allowlist)        S1
├── worktree.rs                every mutating verb from a linked wt hits primary         S2
├── scan_ladder.rs             all 6 merge shapes -> exact MergeStatus, incl. 2 unknowns S3
├── proof_is_sealed.rs         GREP: no Deserialize/Default/From impl for MergedProof    S3
├── purity.rs                  GREP: derive.rs imports no std::fs / std::process         S4
├── transition_table.rs        exhaustive (State x Verb); replay matrix; repair reset    S4
├── doctor_replay.rs           hand-edit fails; legal trail passes; repair recovers      S4
├── cache_wipe.rs              rm -rf cache/ changes nothing but freshness stamps        S4
├── lifecycle.rs               new->start->ship->real squash merge->scan->done           S5
├── invariants_rules.rs        prime stdout starts_with rules stdout, over N scopes      S6
├── setup_hooks.rs             foreign-hook preservation; core.hooksPath; .d/ dispatch   S7
├── board.rs                   BoardModel insta snapshots (filters written day one)      S8
├── json_matrix.rs             every_command_supports_json, walked off the clap tree     S8
├── cli_well_formed.rs         Cli::command().debug_assert()                             F
└── cli_smoke.rs               shells the real binary: argv, exit codes, `ks` alias      F
```

---

## 2. The shared contract — verbatim Rust

> Everything in §2 is **frozen after wave 0**. A missing type or flag is a *request to F*, batched
> between waves — never a direct edit. Compiled and tested as written.

### 2.1 `src/error.rs` — 8 closed shapes, non-empty fix by TYPE

```rust
/// Non-empty BY TYPE: head + tail. `debug_assert` is deleted by `--release`;
/// this is not. ✅ fixes Skeleton-First `Fixes(pub Vec<String>)` and One Gate `Fix::many`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Fix(String);
impl Fix {
    pub fn cmd(s: impl Into<String>) -> Fix { Fix(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Fixes(Fix, Vec<Fix>);
impl Fixes {
    pub fn one(f: Fix) -> Fixes { Fixes(f, Vec::new()) }
    pub fn new(head: Fix, rest: Vec<Fix>) -> Fixes { Fixes(head, rest) }
    pub fn iter(&self) -> impl Iterator<Item = &Fix> {
        std::iter::once(&self.0).chain(self.1.iter())
    }
}
#[macro_export] macro_rules! fix   { ($($t:tt)*) => { $crate::error::Fix::cmd(format!($($t)*)) } }
#[macro_export] macro_rules! fixes { ($h:expr $(, $r:expr)* $(,)?) => {
    $crate::error::Fixes::new($h, vec![$($r),*]) }; }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvCode { NotARepo, NotInitialized, LockHeld, AmbiguousWorktree, GitMissing }

/// Structured payloads for the TWO errors whose output quality is the product.
/// Agents constructing a new refusal use `GateDetail::Plain` and never edit this file.
#[derive(Debug, Serialize)]
#[serde(tag = "detail", rename_all = "snake_case")]
pub enum GateDetail {
    Plain,
    NotLanded { trace: Vec<crate::git::RungTrace> },
    Undispositioned { items: Vec<crate::ids::ItemRef> },
}

#[derive(Debug, thiserror::Error)]
pub enum KsError {
    #[error("{id} is {from}, not {}, — cannot {verb}", crate::transitions::verbs_str(*allowed))]
    IllegalTransition { id: TicketId, from: State, verb: Verb,
                        allowed: &'static [Verb], fix: Fixes },
    #[error("{kind} {id} not found")]
    NotFound   { kind: &'static str, id: String, fix: Fixes },
    #[error("{message}")]
    Gate       { code: GateCode, message: String, detail: GateDetail, fix: Fixes },
    #[error("{message}")]
    Environment{ code: EnvCode, message: String, fix: Fixes },
    #[error("{message}")]
    Git        { message: String, cmd: String, exit: i32, fix: Fixes },
    #[error("{message}")]
    Conflict   { message: String, fix: Fixes },   // duplicate claim, id collision
    #[error("{message}")]
    Invalid    { message: String, fix: Fixes },   // bad args, bad YAML, bad id
    #[error("internal error: {source}")]
    Internal   { #[source] source: anyhow::Error, fix: Fixes },
}

impl KsError {
    pub fn gate(code: GateCode, message: impl Into<String>, fix: Fixes) -> KsError {
        KsError::Gate { code, message: message.into(), detail: GateDetail::Plain, fix }
    }
    /// ✅ There is NO `#[from] std::io::Error`. A bare `?` on a file op cannot
    /// produce a fix-less error; every call site converts deliberately.
    pub fn internal(e: impl Into<anyhow::Error>) -> KsError {
        KsError::Internal { source: e.into(), fix: fixes![fix!("kanspec doctor")] }
    }
    /// TOTAL over the enum — every variant, including Internal. Invariant 9.
    pub fn fixes(&self) -> &Fixes { /* one match, 8 arms */ }
    pub fn kind(&self) -> &'static str;             // stable JSON discriminator
    pub fn detail(&self) -> Option<&GateDetail>;
    pub fn exit_code(&self) -> u8 {
        match self {
            KsError::Environment { .. } => code::ENVIRONMENT,   // 69
            KsError::Internal { .. }    => code::INTERNAL,      // 70
            _                           => code::VIOLATION,     // 1
        }
    }
    pub fn render(&self, mode: &OutMode);           // human -> stderr; json -> stdout envelope
    pub fn to_json(&self) -> serde_json::Value;     // {"ok":false,"error":{kind,message,detail,fix,exit}}
}
pub type Result<T> = std::result::Result<T, KsError>;

pub mod code {
    pub const OK: u8 = 0;
    pub const VIOLATION: u8 = 1;    // gate refusal / invariant violation / doctor
    pub const BLOCK: u8 = 2;        // landcheck Stop hook ONLY (see BlockToken)
    pub const USAGE: u8 = 64;       // clap's native 2 is REMAPPED here
    pub const ENVIRONMENT: u8 = 69; // no repo / no .kanspec/ / lock held / git missing
    pub const INTERNAL: u8 = 70;
}
```

**Error rendering** (`KsError::render`, human mode, stderr):

```
✗ t-9c41 is review, not doing — cannot ship
  → kanspec show t-9c41
```

Exit-2 seal — declared **inside `cmd/landcheck.rs`** so `pub(in ...)` is legal (§2.7):

```rust
// src/cmd/landcheck.rs
pub struct BlockToken(());              // private field: only this file mints one
pub enum Status { Ok, Violation, Block(BlockToken) }
impl Status { pub fn code(self) -> u8 { match self {
    Status::Ok => 0, Status::Violation => 1, Status::Block(_) => 2 } } }
```

### 2.2 `src/ids.rs` — per-kind newtypes, minted under the lock

```rust
macro_rules! id_kind {
    ($name:ident, $prefix:literal, $noun:literal) => {
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            pub const PREFIX: &'static str = $prefix;
            pub const NOUN:   &'static str = $noun;
            /// Accepts `t-9c41` and bare `9c41`; rejects a foreign prefix BY TYPE.
            /// Accepts >= 4 hex so `id_width` can grow without breaking old ids.
            pub fn parse(s: &str) -> Result<Self>;
            pub fn as_str(&self) -> &str;
        }
        impl std::fmt::Display for $name { /* writes the prefixed form */ }
        impl TryFrom<String> for $name { type Error = String; /* .. */ }
        impl From<$name> for String     { /* .. */ }
    };
}
id_kind!(TicketId,   "t-",  "ticket");
id_kind!(ProposalId, "p-",  "proposal");
id_kind!(DecisionId, "D-",  "decision");
id_kind!(QuirkId,    "q-",  "quirk");
id_kind!(CommentId,  "cm-", "comment");

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpecName(String);
impl SpecName { pub fn parse(s: &str) -> Result<SpecName>; pub fn as_str(&self) -> &str; }

/// Visible-text anchors (invariant 5): `p-7de2#c3`, `auth#lockout`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemKind { Change, Prescription, Ticket }          // c / p / t
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemRef { pub proposal: ProposalId, pub kind: ItemKind, pub n: u16 }
impl ItemRef { pub fn parse(s: &str) -> Result<ItemRef>; }  // "p-7de2#c3"
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleRef { pub spec: SpecName, pub rule: String }  // "auth#lockout"
impl RuleRef { pub fn parse(s: &str) -> Result<RuleRef>; }

/// Minting is check-and-retry against the FRESH in-lock snapshot's id set —
/// which includes `closed_ids`. 16 bits is 65536, so birthday collisions bite
/// near ~300 entities; collision-freedom is a property of EXCLUSION, not entropy.
/// After 64 rejections the width steps to 5 hex, which is why `parse` accepts >= 4.
pub struct Minter<'s> { taken: &'s HashSet<String>, seed: u64, width: usize }
impl<'s> Minter<'s> {
    /// Only `Store::transact` constructs one; `seed` comes from KANSPEC_ID_SEED in tests.
    pub(crate) fn new(taken: &'s HashSet<String>, seed: u64, width: usize) -> Minter<'s>;
    pub fn ticket(&self, title: &str)   -> Result<TicketId>;
    pub fn proposal(&self, title: &str) -> Result<ProposalId>;
    pub fn decision(&self, title: &str) -> Result<DecisionId>;
    pub fn quirk(&self, title: &str)    -> Result<QuirkId>;
    pub fn comment(&self, body: &str)   -> Result<CommentId>;
}
pub fn slug(title: &str) -> String;   // "Rate-limit login" -> "rate-limit-login", <= 40 chars
```

### 2.3 `src/keys.rs` — invariant 1 as an absent variant ✅

```rust
pub trait FmKey: Copy + PartialEq + 'static {
    const ORDER: &'static [Self];            // canonical schema order, for INSERTION only
    fn as_str(self) -> &'static str;
}

/// Every writable ticket frontmatter key. There is no `Merged`, no `InMain`,
/// no `Landed`, no `Ready`, no `Ci`. Every write in the crate passes through
/// `Key`, so "no command or UI action can set merge state" is a type fact, not a
/// deny-list. ✅ fixes Skeleton-First's `set_fields(&[(&str, Yv)])` and One
/// Gate's `Op::Transition.also: Vec<(&'static str, Yv)>`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketKey {
    Id, Title, State, Spec, Proposal, Item, Deps, FollowupOf, DiscoveredIn,
    Branch, Worktree, ClaimedBy, Pr, Head, SpecUnchanged, Created,
}
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
pub enum SpecKey     { Feature, Code, StaleAck }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
pub enum ProposalKey { Id, Title, Status, Specs, Approved, Ledger, Created }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
pub enum DecisionKey { Id, Title, Status, Date, Source, Scope, Supersedes, SupersededBy }
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
pub enum QuirkKey    { Id, Title, Paths, Severity, Status, Source, FixedBy }

impl FmKey for TicketKey {
    const ORDER: &'static [TicketKey] = &[ /* exactly the DESIGN.md ticket order */ ];
    fn as_str(self) -> &'static str { /* "followup_of", "discovered_in", "spec_unchanged", .. */ }
}
// ... identical impls for SpecKey / ProposalKey / DecisionKey / QuirkKey

/// The single key type that crosses a module boundary.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(untagged)]
pub enum Key {
    Ticket(TicketKey), Spec(SpecKey), Proposal(ProposalKey),
    Decision(DecisionKey), Quirk(QuirkKey),
}
impl Key {
    pub fn as_str(self) -> &'static str;
    pub fn order(self) -> &'static [&'static str];   // fm::set's insertion-point hint
}

/// READ-side deny-list. A file that ARRIVES with `merged: true` — a hand-edit,
/// an import, a bad merge — has no write path to blame, so `doctor` scans every
/// entity's `#[serde(flatten)] extra` map against this. ✅ the hand-edit vector
/// no type and no grep can cover.
pub const RESERVED_DERIVED: &[&str] = &[
    "merged", "merged_at", "in_main", "landed", "ready", "blocked",
    "stalled", "settling", "stale", "dwell", "ci", "checked_at", "method",
];
```

### 2.4 `src/transitions.rs` — THE legality oracle (one definition)

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State { Todo, Doing, Review, Done, Dropped }
impl State {
    pub const fn glyph(self) -> &'static str;   // ○ ◐ ◈ ● ✕
    pub const fn as_str(self) -> &'static str;  // the frontmatter text, exactly
    pub const fn terminal(self) -> bool { matches!(self, State::Done | State::Dropped) }
}
impl std::fmt::Display for State { /* as_str */ }

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verb { New, Start, Ship, Done, Park, Drop, Confirm, Repair }
impl Verb {
    pub const fn as_str(self) -> &'static str;  // the `## Log` spelling — NOT a command

    /// The verb's REAL invocation, mandatory flags included. `as_str` is the LOG
    /// spelling and five of the eight verbs are not invoked that way: `confirm` is
    /// an option on `scan`, `park`/`drop`/`repair`/`scan --confirm` all require
    /// `--why`, and `new` takes a title rather than an id. Round-E fix: `require`
    /// built its second fix by lowercasing the verb and emitted `kanspec confirm
    /// <id>`, which exits 64. Exhaustive match, no wildcard — a ninth `Verb` cannot
    /// compile until someone writes down how it is actually run.
    pub fn command(self, id: &TicketId) -> String;          // uses cli::invoked_as()
    pub fn command_as(self, ks: &str, id: &TicketId) -> String;  // test seam
}
impl std::fmt::Display for Verb { /* as_str */ }

pub const ALL_STATES: &[State] = &[State::Todo, State::Doing, State::Review,
                                   State::Done, State::Dropped];
pub const ALL_VERBS:  &[Verb]  = &[Verb::New, Verb::Start, Verb::Ship, Verb::Done,
                                   Verb::Park, Verb::Drop, Verb::Confirm, Verb::Repair];

/// A const table + exhaustive match, NOT typestate — an explicit pushback on
/// DESIGN.md's build-plan prose. Ticket state arrives from a hand-editable FILE
/// at runtime, so compile-time phases would be a lie requiring a fallible
/// downcast at every boundary. `from == None` means "does not exist yet", so
/// `New` lives in the same table and `replay` needs no genesis special case.
///
/// `Confirm` and `Repair` ARE in the table. ✅ fixes Skeleton-First (declared,
/// absent from LEGAL -> every `scan --confirm` wedges a ticket) and One Gate
/// (no `Confirm` variant at all while specifying a confirm Log line).
pub const fn next(from: Option<State>, verb: Verb) -> Option<State> {
    use State::*; use Verb as V;
    match (from, verb) {
        (None,          V::New)   => Some(Todo),
        (Some(Todo),    V::Start) => Some(Doing),
        (Some(Review),  V::Start) => Some(Doing),      // rework
        (Some(Doing),   V::Ship)  => Some(Review),
        (Some(Doing),   V::Park)  => Some(Todo),
        (Some(Review),  V::Done)  => Some(Done),       // requires MergedProof
        (Some(Doing),   V::Done)  => Some(Done),       // requires NoCodeWaiver
        (Some(Todo), V::Drop) | (Some(Doing), V::Drop) | (Some(Review), V::Drop) => Some(Dropped),
        (Some(s), V::Confirm) => Some(s),              // non-transition, recorded
        (Some(s), V::Repair)  => Some(s),              // non-transition, recorded
        _ => None,
    }
}
pub fn allowed_from(from: Option<State>) -> Vec<Verb>;
pub fn verbs_str(vs: &[Verb]) -> String;

pub fn require(id: &TicketId, from: State, verb: Verb) -> Result<State>;   // else IllegalTransition

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "break", rename_all = "snake_case")]
pub enum LogViolation {
    Empty,
    NoGenesis     { first: Verb },
    IllegalStep   { index: usize, from: Option<State>, verb: Verb },
    StateMismatch { index: usize, logged: State, legal: State },
    OutOfOrder    { index: usize, at: DateTime<Utc> },
    Divergence    { replayed: State, frontmatter: State },
}

/// THE MECHANICAL PROOF (invariant 10). Folds the ticket's own `## Log` through
/// the SAME oracle the write path uses. Checks THREE things the losing entries
/// each missed one of: legal sequence, each entry's RECORDED state against the
/// legal successor (a doctored log LINE, not just a doctored frontmatter field),
/// and timestamp monotonicity.
///
/// ROUND-B CORRECTION — two passes, and the split is load-bearing. Legality folds
/// from the LAST `Verb::Repair`; the clock is checked over the WHOLE log with no
/// repair exemption. Folding strictly forward (returning at the first bad entry,
/// never reaching a later reset) left `repair` unable to rescue an `IllegalStep`
/// or a `StateMismatch` — which is exactly what an import and a union-merged
/// `## Log` (two `start` lines) produce — and since `transact` step 8 re-proves
/// the STAGED bytes with no repair exemption, the repair could not even be
/// written: the ticket was permanently unwritable, the precise outcome D-12
/// exists to prevent. Verified end-to-end in round B against a real repo.
/// Conversely the clock stays global, because a `## Log` is a chronological
/// record and an appended attestation must never launder a temporal anomaly —
/// otherwise the one thing that binds the human (R-2) is forgeable by a line at
/// the bottom. An out-of-order pair therefore stays a refusal whatever follows
/// it, and its fix is the safe mechanical one the message names: reorder the
/// lines. Attesting a STATE is a human's to do; rewriting WHEN is not.
pub fn replay(entries: &[LogEntry]) -> std::result::Result<State, LogViolation> {
    // pass 1 — the clock, over every entry, no repair exemption
    let mut last: Option<DateTime<Utc>> = None;
    for (i, e) in entries.iter().enumerate() {
        if last.is_some_and(|prev| e.at < prev) {
            return Err(LogViolation::OutOfOrder { index: i, at: e.at });
        }
        last = Some(e.at);
    }
    // pass 2 — legality, from the last attested reset
    let start = entries.iter().rposition(|e| e.verb == Verb::Repair).unwrap_or(0);
    let mut cur: Option<State> = None;
    for (i, e) in entries.iter().enumerate().skip(start) {
        cur = match e.verb {
            // Repair is the ONE verb whose logged state is authoritative rather
            // than derived: the human-attested reset that makes an imported or
            // already-broken repo RECOVERABLE instead of permanently unwritable.
            Verb::Repair => Some(e.state),
            v => {
                // `next` has exactly one `(None, verb)` entry, so a log opening
                // with anything else is a MISSING GENESIS. Round B added this
                // branch: without it `NoGenesis` was unconstructible dead code
                // and the break surfaced as `IllegalStep { from: None }`, which
                // is not wording a human can act on.
                if cur.is_none() && v != Verb::New {
                    return Err(LogViolation::NoGenesis { first: v });
                }
                let n = next(cur, v)
                    .ok_or(LogViolation::IllegalStep { index: i, from: cur, verb: v })?;
                if n != e.state {
                    return Err(LogViolation::StateMismatch { index: i, logged: e.state, legal: n });
                }
                Some(n)
            }
        };
    }
    cur.ok_or(LogViolation::Empty)
}

/// Called per touched ticket by `Store::transact` on commit AND by `doctor`.
pub fn prove(t: &Ticket) -> std::result::Result<(), LogViolation>;
```

### 2.5 `src/logentry.rs` — the `## Log` grammar, one place

```
- 2026-08-30T14:20Z  doing    claude/sess-a91      kanspec start (branch + worktree created)
  └ %Y-%m-%dT%H:%MZ  state    actor (<=20, padded) verb + optional " (note)"
```

```rust
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LogEntry {
    pub at: DateTime<Utc>,
    /// The resulting state. STORED, not a format-time parameter — so
    /// `parse(format(e)) == e` and `replay` can catch a line whose printed state
    /// disagrees with its verb. ✅ fixes Sealed Keel's stateless LogEntry.
    pub state: State,
    pub actor: String,          // Actor::label()
    pub verb: Verb,
    pub note: Option<String>,
}
impl LogEntry {
    pub fn format(&self) -> String;
    /// `None` == not a log line, so prose under `## Log` survives untouched.
    pub fn parse(line: &str) -> Option<LogEntry>;
}
/// Extracts every parseable entry under the `## Log` heading of a ticket body.
pub fn parse_log(body: &str) -> Vec<LogEntry>;
pub const LOG_HEADING: &str = "## Log";
pub const STEPS_HEADING: &str = "## Steps";
```

### 2.6 `src/ctx.rs` — built ONCE, and provably `Send + Sync` ✅

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    Human { name: String },
    Agent { session: String, tool: String },
}
impl Actor {
    /// KANSPEC_ACTOR + KANSPEC_ACTOR_KIND (tests) > CLAUDE_SESSION_ID/CURSOR_SESSION_ID/
    /// CODEX_SESSION_ID (agent) > git config user.email > $USER (human).
    pub fn detect() -> Actor;
    pub fn label(&self) -> String;     // "trevor" | "claude/sess-a91"
    pub fn is_agent(&self) -> bool;
}

/// Invariant 8, mechanically. Private field; the ONLY constructor refuses an
/// `Actor::Agent`, and `plan_accept`/`plan_revoke` take `&HumanActor`. An agent
/// session literally cannot call them. ✅ unenforced in all three inputs.
pub struct HumanActor(Actor);
impl HumanActor {
    pub fn require(a: &Actor, verb: &'static str) -> Result<HumanActor>;
    pub fn actor(&self) -> &Actor;
}

#[derive(Clone, Copy, Debug)]
pub enum OutMode { Human { color: bool }, Json }

/// NOTE what is absent: no `Ui`, no `RefCell`, no `Rc`, no handle to stdout.
/// Presentation lives in `out.rs`. That is what makes `Arc<Ctx>` crossable into
/// `spawn_blocking`. ✅ fixes One Gate's `RefCell<Ui>` (verified E0277).
pub struct Ctx {
    pub repo:   Repo,
    pub layout: Layout,
    pub cfg:    Config,
    pub git:    Git,
    pub gh:     Gh,
    pub actor:  Actor,
    pub now:    DateTime<Utc>,   // KANSPEC_NOW-overridable => deterministic goldens
    pub out:    OutMode,
    pub invoked_as: &'static str,
}
impl Ctx {
    pub fn open(cli: &Cli, cwd: &Path) -> Result<Ctx>;
    pub fn snapshot(&self) -> Result<Snapshot> { crate::store::load_snapshot(self) }
    pub fn invocation(&self) -> String;         // "kanspec ship --pr 142" for the Log note
    pub fn style(&self) -> Style;
}
const _: fn() = || { fn need<T: Send + Sync + 'static>() {} need::<std::sync::Arc<Ctx>>(); };
```

### 2.7 `src/paths.rs` — worktree unification, structurally unavoidable

```rust
pub struct Repo {                 // ALL FIELDS PRIVATE ✅ (both losers exposed primary_root)
    primary_root: PathBuf, git_dir: PathBuf, common_dir: PathBuf,
    here: PathBuf, linked: bool,
}
impl Repo {
    /// ONE `git rev-parse --path-format=absolute --git-common-dir --git-dir --show-toplevel`
    /// (via std::process::Command — `Git` needs a root, so discovery cannot use it).
    ///   1. exit 128                -> Environment{NotARepo}
    ///   2. git_dir == common_dir   -> primary_root = show_toplevel   [survives --separate-git-dir]
    ///   3. else                    -> first `worktree <path>` stanza of
    ///                                 `git worktree list --porcelain -z`
    ///   4. sanity: primary_root's gitdir must resolve to common_dir, else
    ///      Environment{AmbiguousWorktree} — a typed refusal, never a guess.
    pub fn discover(cwd: &Path, repo_flag: Option<&Path>) -> Result<Repo>;
    pub fn primary_root(&self) -> &Path;
    pub fn here(&self) -> &Path;          // where the user stands — for `where`, hooks, branch
    pub fn common_dir(&self) -> &Path;
    pub fn git_dir(&self) -> &Path;
    pub fn linked(&self) -> bool;
}

/// Private field, no `From<PathBuf>`, no public constructor. `Layout` is the
/// only thing that can name a kanspec file, and `Repo::discover` is the only
/// source of one. "Every command resolves git-common-dir" is therefore not a
/// rule anyone can forget — there is no other way to name a file. ✅
pub struct KanspecDir(PathBuf);
impl KanspecDir {
    pub(crate) fn resolve(repo: &Repo) -> KanspecDir;   // <primary_root>/.kanspec
    pub fn display(&self) -> std::path::Display<'_>;
    pub fn exists(&self) -> bool;
    fn join(&self, s: impl AsRef<Path>) -> PathBuf;     // PRIVATE
}

pub struct Layout { ks: KanspecDir, features_md: PathBuf, architecture_md: PathBuf }
impl Layout {
    /// Non-circular: `KanspecDir::resolve` needs only `Repo`; `Config::load`
    /// needs only `KanspecDir`; `Layout::open` needs both. ✅ fixes
    /// Skeleton-First's `Layout::open(repo, &cfg)` circularity.
    pub(crate) fn open(repo: &Repo, cfg: &Config) -> Layout;
    pub fn ks(&self) -> &KanspecDir;
    pub fn config_toml(&self)      -> PathBuf;   // .kanspec/config.toml
    pub fn tickets_dir(&self)      -> PathBuf;
    pub fn ticket(&self, id: &TicketId) -> PathBuf;
    pub fn specs_dir(&self)        -> PathBuf;
    pub fn spec(&self, n: &SpecName) -> PathBuf;
    pub fn decisions_dir(&self)    -> PathBuf;
    pub fn decision(&self, id: &DecisionId) -> PathBuf;
    pub fn quirks_dir(&self)       -> PathBuf;
    pub fn quirk(&self, id: &QuirkId) -> PathBuf;
    pub fn proposals_dir(&self)    -> PathBuf;
    pub fn proposals_closed_dir(&self) -> PathBuf;
    pub fn proposal_dir(&self, id: &ProposalId, slug: &str) -> PathBuf;
    pub fn proposal_md(&self, dir: &Path) -> PathBuf;
    pub fn comments_jsonl(&self, dir: &Path) -> PathBuf;
    pub fn cache_dir(&self)        -> PathBuf;
    pub fn lock(&self)             -> PathBuf;   // cache/lock
    pub fn gitstate(&self)         -> PathBuf;   // cache/gitstate.json
    pub fn features_md(&self)      -> &Path;     // honours [paths]
    pub fn architecture_md(&self)  -> &Path;
    pub fn path_for(&self, e: &EntityRef) -> PathBuf;   // the Op applier's dispatch
}
```

**Why every seal is colocated with its minting module.** `pub(in path)` requires `path` to be an
*ancestor*. Reproduced in-environment:

```
error[E0742]: visibilities can only be restricted to ancestor modules
```

for `pub(in crate::git) fn mint()` on an item declared in `crate::model`. In this flat layout each
sealed type lives **in the file that mints it** (`Sha`/`HeadSha` in `git.rs`; `MergedProof`,
`NoCodeWaiver`, `ScanToken`, `Detection` in `scan.rs`; `LockToken` in `lock.rs`; `BlockToken` in
`cmd/landcheck.rs`), sealed by a plain private field. This compiles; ✅ Sealed Keel's five
`pub(in ...)` seals do not.

### 2.8 `src/config.rs`

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub main: String,                 // "origin/main"; resolved via symbolic-ref if unset
    pub id_width: usize,              // 4
    pub sync: SyncMode,               // Batch (default) | Commit | Branch(v0.4)
    pub port: u16,                    // 5757
    pub branch_prefix: String,        // "ks/"
    pub worktree_dir: PathBuf,        // "../kanspec-wt"
    pub lock_timeout_secs: u64,       // 5
    pub paths: Paths,
    pub windows: Windows,
    pub git: GitCfg,
    pub ci: CiCfg,
    pub hooks: HooksCfg,
}
#[derive(Clone, Debug, Deserialize, Serialize)] #[serde(default, deny_unknown_fields)]
pub struct Paths { pub features: PathBuf, pub architecture: PathBuf }      // KANSPEC-*.md
#[derive(Clone, Copy, Debug, Deserialize, Serialize)] #[serde(default, deny_unknown_fields)]
pub struct Windows {
    pub stall_secs: u64,              // 7200  (doing, no commit/update)
    pub review_dwell_secs: u64,       // 604800
    pub in_main_dwell_secs: u64,      // 86400
    pub settling_dwell_secs: u64,     // 259200
    pub discovered_dwell_secs: u64,   // 604800
    pub stale_merges: u32,            // 3     (spec staleness tripwire)
    pub fetch_max_age_secs: u64,      // 300
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode { Batch, Commit, Branch }
#[derive(Clone, Debug, Deserialize, Serialize)] #[serde(default, deny_unknown_fields)]
pub struct GitCfg  { pub fetch: bool, pub gh: GhMode }
#[derive(Clone, Debug, Deserialize, Serialize)] #[serde(default, deny_unknown_fields)]
pub struct CiCfg   { pub provider: CiProvider, pub homerunner: HomerunnerCfg }
#[derive(Clone, Debug, Deserialize, Serialize)] #[serde(default, deny_unknown_fields)]
pub struct HooksCfg{ pub landcheck: bool }        // v0.2, DEFAULT FALSE (§11 D-14)

impl Config {
    /// A MISSING file is `Config::default()`, not an error — `.kanspec/` without
    /// a config.toml is legal. A malformed one is `Invalid` with the toml span.
    pub fn load(ks: &KanspecDir) -> Result<Config>;
    pub fn render_default() -> String;             // what `init` writes, with comments
}
```

### 2.9 `src/model.rs` — types only, zero IO

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct TicketFm {
    pub id: TicketId, pub title: String, pub state: State,
    pub spec: Option<SpecName>, pub proposal: Option<ProposalId>, pub item: Option<String>,
    #[serde(default)] pub deps: Vec<TicketId>,
    pub followup_of: Option<TicketId>, pub discovered_in: Option<TicketId>,
    pub branch: Option<String>, pub worktree: Option<PathBuf>,
    pub claimed_by: Option<String>, pub pr: Option<u64>,
    /// A plain String: read from git by `ship`/`done`, never typed by an agent —
    /// enforced upstream, because the only value that can be WRITTEN here comes
    /// from `HeadSha` (§2.11), which only `git.rs` can mint.
    pub head: Option<String>,
    pub spec_unchanged: Option<String>,
    pub created: DateTime<Utc>,
    /// Load-bearing: a key written by a NEWER kanspec is never dropped by an
    /// older one, and `doctor::check_reserved_keys` scans exactly this map.
    #[serde(flatten)] pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}
// NOTE what is absent, forever: merged, in_main, ready, stalled, ci, checked_at.

#[derive(Debug, Clone)]
pub struct Ticket {
    pub fm: TicketFm,
    pub path: PathBuf,
    pub body: String,                 // everything after the closing fence, verbatim = TRUTH
    pub steps: Vec<Step>,             // parsed `- [ ]` view of the body
    pub log: Vec<LogEntry>,           // parsed `## Log` view of the body
    pub mtime: SystemTime,
}
#[derive(Debug, Clone, Serialize)] pub struct Step { pub index: usize, pub done: bool, pub text: String }

#[derive(Debug, Clone, Deserialize)] pub struct SpecFm {
    pub feature: String, #[serde(default)] pub code: Vec<String>,
    pub stale_ack: Option<StaleAck>, #[serde(flatten)] pub extra: BTreeMap<String, serde_yaml_ng::Value> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct StaleAck {
    pub sha: String, pub at: DateTime<Utc>, pub by: String, pub why: String }
#[derive(Debug, Clone)] pub struct Spec {
    pub name: SpecName, pub fm: SpecFm, pub path: PathBuf, pub body: String, pub rules: Vec<Rule> }
#[derive(Debug, Clone, Serialize)] pub struct Rule {
    pub anchor: String,                       // "auth.lockout" -> RuleRef "auth#lockout"
    pub text: String, pub provenance: Vec<ProposalId>, pub line: usize }

#[derive(Debug, Clone, Deserialize)] pub struct DecisionFm { /* DESIGN.md decision frontmatter */ }
#[derive(Debug, Clone)] pub struct Decision { pub fm: DecisionFm, pub path: PathBuf,
    pub body: String, pub scope: Vec<String> }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")] pub enum DecisionStatus { Proposed, Accepted, Superseded, Revoked }

#[derive(Debug, Clone, Deserialize)] pub struct QuirkFm { /* DESIGN.md quirk frontmatter */ }
#[derive(Debug, Clone)] pub struct Quirk { pub fm: QuirkFm, pub path: PathBuf, pub body: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")] pub enum Severity { Landmine, Gotcha, Debt }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")] pub enum QuirkStatus { Active, Fixed, Stale }

// v0.2 types, declared in wave 0 so V2 never edits this file:
#[derive(Debug, Clone, Deserialize)] pub struct ProposalFm { /* .. */ }
#[derive(Debug, Clone)] pub struct Proposal { pub fm: ProposalFm, pub dir: PathBuf,
    pub body: String, pub items: Vec<Item> }
#[derive(Debug, Clone, Serialize)] pub struct Item { pub id: ItemRef, pub text: String,
    pub prescription: Option<Prescription> }
#[derive(Debug, Clone, Serialize)] pub enum Prescription {
    TempUntil(TicketId), Promote(PromoteAs), Untyped }
#[derive(Debug, Clone, Copy, Serialize)] pub enum PromoteAs { Decision, Spec, Quirk }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct CommentOp { /* DESIGN.md jsonl row */ }

/// Everything on disk, loaded once, plus the disposable cache. Derived state is
/// computed FROM this and never stored IN it — the whole design in one struct.
pub struct Snapshot {
    pub tickets:   BTreeMap<TicketId, Ticket>,
    /// OPEN proposals ONLY. Closed proposal BODIES are never read from disk, so
    /// there is physically no value through which closed prose can reach
    /// `rulesdoc::build` or `prime`. Invariant 4 is a property of the
    /// generator's INPUT TYPE. ✅ (Sealed Keel and Skeleton-First both load them.)
    pub proposals: BTreeMap<ProposalId, Proposal>,
    /// Ids of closed proposals — for collision-free minting and NOTHING else.
    /// Non-obvious: omitting closed BODIES reopens an id collision against
    /// `proposals/closed/` unless the ids are tracked separately. ✅
    pub closed_ids: HashSet<String>,
    pub specs:     BTreeMap<SpecName, Spec>,
    pub decisions: BTreeMap<DecisionId, Decision>,
    pub quirks:    BTreeMap<QuirkId, Quirk>,
    pub comments:  BTreeMap<ProposalId, Vec<CommentOp>>,   // id+op deduped on read
    pub git:       GitState,          // the gitignored cache — the SOLE home of derived git facts
    pub cfg:       Config,
    pub now:       DateTime<Utc>,
    /// Bumped on every successful `transact`, and carried on `BoardModel.rev`.
    /// Present from day one so the memoization escape hatch is a one-file change
    /// later, not a re-architecture.
    ///
    /// ROUND-D CORRECTION: the server does **not** memoize on it (D-40), and the SSE
    /// `Tick.rev` is a different number — `up`'s own per-run generation counter, bumped
    /// once per settled batch. The two are never compared: D-23 makes the tick a bare
    /// "something moved" signal and the SPA refetches `/api/board` regardless.
    pub rev:       u64,
}
impl Snapshot {
    pub fn ticket(&self, id: &TicketId) -> Result<&Ticket>;      // else NotFound + fix
    pub fn spec(&self, n: &SpecName)    -> Result<&Spec>;
    pub fn decision(&self, id: &DecisionId) -> Result<&Decision>;
    pub fn quirk(&self, id: &QuirkId)   -> Result<&Quirk>;
    pub fn taken_ids(&self) -> HashSet<String>;                  // incl. closed_ids
}
```

### 2.10 `src/plan.rs` — the typed edit vocabulary

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum EntityRef {
    Ticket(TicketId), Proposal(ProposalId), Spec(SpecName),
    Decision(DecisionId), Quirk(QuirkId),
}

pub enum Op {
    /// The ONLY op that can touch `state:`. It computes the destination itself
    /// via `next(current, verb)` — the caller does NOT pass a destination
    /// ✅ (Skeleton-First's `transition(.., to: State, ..)` let a caller pass a
    /// state the table never produces) — and emits the frontmatter delta AND the
    /// `## Log` line in ONE staged write to ONE file, so the omission of the log
    /// append is unavailable rather than merely rejected.
    Transition { id: TicketId, verb: Verb, actor: Actor, at: DateTime<Utc>,
                 detail: String, also: Vec<(TicketKey, Yv)> },
    /// Non-state fields on any entity. `Key` is closed, so a derived key cannot
    /// be NAMED here, let alone written.
    SetFields  { entity: EntityRef, sets: Vec<(Key, Yv)> },
    CreateEntity { entity: EntityRef, contents: String },   // errors if the file exists
    AppendSection{ entity: EntityRef, heading: &'static str, line: String },
    MarkSteps  { id: TicketId, checks: Vec<(usize, bool)> },  // `done` triage: actually-done
    AppendJsonl{ path: PathBuf, line: String },              // comments.jsonl (v0.2)
    MoveDir    { from: PathBuf, to: PathBuf },               // close: -> proposals/closed/
    /// Generated bytes at a path the tracker does not own: the `KANSPEC-*.md`
    /// projections (D-20) and, since ROUND D, `board --export <file>` (D-39).
    /// The only op that writes arbitrary bytes to an arbitrary path — safe because
    /// `writes_tracked_file()` is false for it and it cannot name a `Key` at all.
    WriteGenerated { path: PathBuf, contents: String },
    /// cache/gitstate.json. `ScanToken` is minted solely by `scan::scan_all`, so
    /// "written by scan and nothing else" is a type fact — and the write happens
    /// INSIDE the lock. ✅ (One Gate allowlisted cache.rs for unlocked writes.)
    WriteGitState { token: ScanToken, state: GitState },
}
impl Op {
    pub fn entity(&self) -> Option<&EntityRef>;
    /// ROUND-B ADDITION, WIDENED IN ROUND C. False for `WriteGitState` — `cache/`
    /// is gitignored, disposable, and rebuilt from nothing by the next `scan` —
    /// and false for `WriteGenerated` too. `Store::transact` step 9 reads this so
    /// `sync = "commit"` has nothing to commit for a scan (§2.13).
    ///
    /// `WriteGenerated` is excluded for the same reason arrived at from the other
    /// side: it writes `KANSPEC-*.md` at the REPO ROOT, while `commit_kanspec`
    /// scopes all three of its git calls to `:(glob,top).kanspec/**`. Counting it
    /// therefore never commits the projection — it only makes `scan` (which
    /// regenerates them, D-20) sweep a human's pending tracker edit into a commit
    /// labelled after the scan, which is exactly the harm this predicate exists to
    /// prevent. `scan_ladder.rs::a_scan_commits_nothing_under_sync_commit_…` fails
    /// without the arm. If the projections should ever be auto-committed, widen
    /// `commit_kanspec`'s pathspec; do not re-arm this predicate.
    pub fn writes_tracked_file(&self) -> bool;
}

#[derive(Default)]
pub struct Plan { pub ops: Vec<Op>, pub minted: Vec<EntityRef>, pub note: Option<String> }
impl Plan {
    pub fn of(ops: Vec<Op>) -> Plan;
    pub fn empty() -> Plan;
    pub fn push(&mut self, op: Op) -> &mut Plan;
    /// Belt-and-braces (the key enums already make it unreachable), plus id
    /// uniqueness and one-transition-per-ticket-per-plan.
    pub fn validate(&self, snap: &Snapshot) -> Result<()>;
    pub fn touched(&self, layout: &Layout) -> Vec<PathBuf>;
}
```

### 2.11 `src/git.rs` — shell-out, tri-state, sealed SHAs

```rust
/// Private field; the ONLY constructor is `Sha::mint`, private to THIS file, and
/// every call site parses real git stdout. `head:` is therefore provably read
/// from git and can never be agent-typed — DESIGN.md's second gear, enforced.
/// `Serialize` yes; `Deserialize` NEVER, so a cache entry cannot mint one. ✅
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha(String);
impl Sha {
    fn mint(s: &str) -> Option<Sha>;        // PRIVATE — >= 7 lowercase hex
    pub fn short(&self) -> &str;            // 7 chars
    pub fn as_str(&self) -> &str;
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct HeadSha(Sha);
impl HeadSha { pub fn sha(&self) -> &Sha; }

/// Always emits `:(glob,top)`. `Git` will not accept a bare &str where a
/// Pathspec is expected, so recon finding 6 (pathspecs are cwd-relative; `glob`
/// makes `**` cross separators exactly like globset) cannot be forgotten. ✅
#[derive(Clone, Debug, PartialEq, Eq)] pub struct Pathspec(String);
impl Pathspec { pub fn glob(g: &str) -> Pathspec; pub fn as_str(&self) -> &str; }

/// "Unknown" is a VALUE, not an error path: every consumer must destructure it,
/// so no code path can quietly fold a git failure into "not merged" (invariant 2).
#[derive(Clone, Debug)] pub enum Tri<T> { Yes(T), No, Unknown(Unknown) }

/// One variant per decline reason, exhaustively matched, each carrying the
/// fields its badge text needs. ✅ (One Gate: `Unknown{code, detail}` strings.)
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Unknown {
    NoHead,
    ZeroCommitBranch,
    HeadNotInObjectStore   { sha: String },
    GitFailed              { cmd: String, code: i32, stderr: String },
    GhUnavailable          { why: String },
    SquashSuspectedNoGh    { plus_lines: usize },
    GhMergedButNotAncestor { merge_sha: String },
    FetchStale             { age_secs: u64 },
    ConflictingSignals     { rungs: Vec<RungTrace> },
}
impl Unknown { pub fn badge(&self) -> String; }   // "unknown (squash suspected, no gh)"

#[derive(Clone, Debug, Serialize)]
pub struct RungTrace { pub method: Method, pub cmd: String, pub exit: i32,
                       pub saw: String, pub verdict: &'static str }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method { Ancestry, GhPr, Trailer, PatchId, HumanConfirm, None }

pub struct Git { root: PathBuf }        // ALWAYS `git -C <primary_root>`
pub struct GitOut { pub code: i32, pub out: String, pub err: String }
#[derive(Clone, Debug, Serialize)] pub struct ChangedPath { pub status: char, pub path: String,
                                                            pub renamed_from: Option<String> }
#[derive(Clone, Debug)] pub struct CherryLine { pub upstream: bool, pub sha: String }
#[derive(Clone, Debug, Serialize)] pub struct WorktreeRow { pub path: PathBuf,
    pub branch: Option<String>, pub head: Option<String>, pub bare: bool,
    pub detached: bool, pub locked: Option<String>, pub prunable: Option<String> }

impl Git {
    pub(crate) fn bind(root: &Path) -> Git;
    /// ALWAYS scrubs GIT_DIR / GIT_WORK_TREE / GIT_INDEX_FILE from the child env:
    /// git sets GIT_DIR when running hooks, and env beats `-C`. `Err` only when
    /// git is not runnable at all; a non-zero exit is a normal `GitOut`.
    pub fn run(&self, args: &[&str]) -> Result<GitOut>;
    pub fn run_ps(&self, args: &[&str], ps: &[Pathspec]) -> Result<GitOut>;

    pub fn head_sha(&self, rev: &str)  -> Result<HeadSha>;
    pub fn current_branch(&self)       -> Option<String>;   // symbolic-ref; None = detached
    pub fn object_exists(&self, s: &Sha) -> bool;           // rev-parse --verify --quiet ^{commit}
    pub fn resolve_main(&self, cfg: &str) -> Result<String>;
    pub fn is_ancestor(&self, s: &Sha, base: &str) -> Tri<()>;      // 0=Yes 1=No 128=Unknown
    pub fn commits_ahead(&self, base: &str, head: &Sha) -> Tri<u32>; // the zero-commit guard
    pub fn grep_trailer(&self, base: &str, id: &TicketId) -> Tri<Vec<Sha>>;
    pub fn cherry(&self, base: &str, head: &Sha) -> Tri<Vec<CherryLine>>;
    pub fn changed_paths(&self, base: &str, head: &str) -> Tri<Vec<ChangedPath>>;  // 3-dot -M -z
    /// `base` is explicit and REQUIRED: the range is `<since>..<base>`, and without a
    /// named ref it would default to HEAD — in the primary worktree, whatever branch the
    /// human happens to be standing on. Same for `last_touch`: the spec's last-edit anchor
    /// must be read on a named ref. (Both corrected in round A from a signature that
    /// omitted the ref while its own comment named it.)
    pub fn merges_touching(&self, since: &Sha, base: &str, globs: &[Pathspec]) -> Tri<u32>; // --first-parent
    pub fn last_touch(&self, rev: &str, p: &Pathspec) -> Option<(Sha, DateTime<Utc>)>;
    pub fn ahead_behind(&self, base: &str, head: &str) -> Option<(u32, u32)>;
    pub fn last_commit_at(&self, rev: &str) -> Option<DateTime<Utc>>;
    pub fn fetch(&self) -> Result<()>;
    pub fn fetch_age(&self) -> Option<Duration>;            // mtime(common_dir/FETCH_HEAD)
    pub fn worktrees(&self) -> Result<Vec<WorktreeRow>>;    // -z; FIRST stanza is primary
    pub fn worktree_add(&self, path: &Path, branch: &str, base: &str) -> Result<()>;
    pub fn worktree_remove(&self, path: &Path, force: bool) -> Result<()>;
    pub fn branch_delete(&self, branch: &str, force: bool) -> Result<()>;
    pub fn dirty_kanspec(&self) -> Result<u32>;             // status --porcelain=v2 -z
    pub fn is_ignored(&self, p: &Path) -> bool;             // check-ignore
    pub fn is_tracked(&self, p: &Path)  -> bool;            // ls-files --error-unmatch
    pub fn hooks_dir(&self) -> Result<PathBuf>;             // --git-path hooks (core.hooksPath!)
    pub fn commit_kanspec(&self, msg: &str) -> Result<()>;  // sync = "commit"
}
```

`src/gh.rs` — **the one and only mock seam in the crate.**

```rust
/// All fields private, so nothing outside `gh.rs` can observe their shape. Three
/// round-A corrections to the original `{ slug: Option<String>, fixtures, authed:
/// OnceCell<bool> }`:
///   * `OnceCell` is `!Sync` and breaks `Arc<Ctx>: Send + Sync` (§2.6) — `OnceLock`
///     memoizes identically and is the `Sync` one.
///   * `mode` is REQUIRED to honour `[git] gh = auto|always|never`, which the original
///     field list omitted; `never` must win over every other path, fixture seam included.
///   * `root` + a `OnceLock` slug deliver the deferral `detect`'s own doc comment
///     promises. A plain `Option<String>` can only be filled eagerly in `detect`, which
///     puts a subprocess in `Ctx::open` for every command — including the ~90% that
///     never ask `gh` anything.
pub struct Gh { root: PathBuf, slug: OnceLock<Option<String>>, mode: GhCfg,
                fixtures: Option<PathBuf>, authed: OnceLock<bool> }
#[derive(Debug, Clone, Serialize)] pub struct GhUnavailable(pub String);
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrInfo { pub number: u64, pub state: PrState, pub merged_at: Option<DateTime<Utc>>,
                    pub merge_commit: Option<String>, pub head_ref_oid: String, pub url: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")] pub enum PrState { Open, Closed, Merged }

impl Gh {
    /// `$KANSPEC_GH_FIXTURES` points at recorded `gh` JSON. You cannot create a
    /// real GitHub PR in a test, and rung 2 is the only rung that catches a
    /// title-only squash — the case that defeats all four. Git itself is NEVER
    /// mocked. ✅ (neither Sealed Keel nor One Gate can test rung 2.)
    pub(crate) fn detect(git: &Git, cfg: &GhCfg) -> Gh;
    pub fn available(&self) -> bool;                            // `gh auth status`, cached
    pub fn pr_view(&self, n: u64) -> std::result::Result<PrInfo, GhUnavailable>;
    /// REQUIRED, not optional: a squash-merged ticket with `pr: null` would
    /// otherwise skip the only rung that can see a title-only squash.
    /// `--state all` — the PR that landed a squash is MERGED, and therefore
    /// invisible to `gh pr list`'s default `--state open`. Sorted merged-first /
    /// newest-merge-first, so §7's literal `.find(|p| p.state == Merged)` picks the
    /// right PR when a branch name has been reused.
    pub fn pr_for_head(&self, branch: &str)
        -> std::result::Result<Vec<PrInfo>, GhUnavailable>;
}

/// Free functions, added in round A. Neither changes a signature above.
///
/// The ONE place a list of PRs becomes a verdict, so "gh answered, and nothing it
/// showed had landed" cannot be spelled two ways. `Some` only for a PR GitHub itself
/// reports MERGED (most recent merge wins on a reused branch name); `None` is
/// inconclusive and NEVER a negative — a closed PR's commits can still have been
/// cherry-picked onto main.
pub fn merged_pr(list: &[PrInfo]) -> Option<&PrInfo>;

/// The fixture seam's file-naming convention as CODE rather than a comment a later
/// slice has to guess: `pr-142`, `head-ks-t-9c41-x` (every character a branch may
/// carry but a flat filename may not is folded to `-`). Feed either straight to
/// `TestRepo::gh_fixture(name, json)`, which appends `.json`.
pub fn fixture_name_pr(n: u64) -> String;
pub fn fixture_name_head(branch: &str) -> String;
```

**A missing fixture is inconclusive, never empty.** A test that forgets to record one
gets `unknown`, never a false "not merged" — the difference the seam exists for.

### 2.12 `src/out.rs` — one rendering layer

```rust
pub struct Style { pub color: bool, pub width: usize, pub quiet: bool }

/// The second and LAST trait in the crate (~25 impls day one). `--json` IS the
/// Serialize impl; there is zero hand-written JSON. Monomorphic: no dyn, no
/// `serde_json::Value` per render, no coherence-trapping blanket impl, and — the
/// decisive property for a parallel build — each command's payload struct and
/// its `impl Render` live in THAT COMMAND'S OWN FILE, so `out.rs` never grows a
/// variant or a match arm. ✅ (One Gate's `Report` mega-enum in a single-owner
/// file; Sealed Keel's `impl<T: Serialize + Paint> View for T`.)
pub trait Render: serde::Serialize {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()>;
}
/// The single emit point. Handlers NEVER print.
pub fn emit<R: Render>(r: &R, mode: &OutMode) -> Result<()>;

/// The shared human primitive: most output is a list of these.
pub struct Line { pub glyph: char, pub id: Option<String>, pub text: String,
                  pub dim: Option<String>, pub fix: Option<String>, pub url: Option<String> }
impl Line { pub fn write(&self, w: &mut dyn Write, st: &Style) -> std::io::Result<()>; }

pub struct Table;                                  // comfy-table preset wrapper
impl Table {
    pub fn new(headers: &[&str], st: &Style) -> comfy_table::Table;
    pub fn kanspec_preset(t: &mut comfy_table::Table, st: &Style);
}
pub fn rel_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String;   // "14m ago", "3h", "2d"
pub fn paint(s: &str, c: Color, st: &Style) -> String;

/// THE ONE PADDING PRIMITIVE. `paint` returns a string that already carries ANSI
/// escapes, so `format!("{:<9}", painted)` counts 14 chars where the terminal
/// shows 6, decides the field is full, and pads NOTHING. That was the round-E
/// #1 defect: every id-bearing command jammed the id into its title and shifted
/// the right-aligned `→ fix` three columns whenever colour was on. Nothing
/// painted may reach a `{:<N}`; it goes through here.
///
/// `visible_len` is `pub` for the same reason — `tests/render_color.rs` is an
/// integration test and reaches both only through the crate's public API.
/// Widening this frozen file by two functions is deliberate and blessed.
pub fn pad_visible(s: &str, width: usize) -> String;   // pads to VISIBLE columns
pub fn visible_len(s: &str) -> usize;                  // SGR-aware; display columns (UAX #11)
/// The command word of a fix, spoken as the binary the user actually typed.
/// `Fix::cmd` routes through `spoken`, so all ~123 hardcoded `fix!("kanspec …")`
/// sites are covered without being edited, and the human and `--json` fix lists
/// cannot disagree. Rewrites ONLY in command position (string start, or after a
/// backtick) so `.kanspec/` paths and `git commit -m "kanspec: sync"` survive.
pub fn spoken(cmd: &str) -> String;
pub fn spoken_as(cmd: &str, ks: &str) -> String;       // the test seam (cf. D-43)
pub enum Color { Red, Green, Yellow, Blue, Cyan, Dim, Bold }
/// Set ONCE in `run()` from `--color`, never from the env dance: recon proved
/// CLICOLOR_FORCE beats NO_COLOR in owo-colors' supports-color.
pub fn apply_color_policy(choice: ColorChoice) -> bool;
```

### 2.13 `src/lock.rs` + `src/store.rs` — THE ONE WRITE PATH

```rust
// ── lock.rs ──────────────────────────────────────────────────────────────────
/// flock(2) via libc. The kernel releases it when the process dies, so there is
/// NO stale-lock reaper to get subtly wrong (pid reuse, clock skew) and `kill -9`
/// mid-transaction cannot wedge the repo. ✅ (Sealed Keel and One Gate both
/// hand-roll "pid dead AND older than 60s".)
///
/// The private field is the WRITE CAPABILITY: every byte-writing primitive in
/// `store` takes `&LockToken`, and `acquire` is the only constructor, so "the
/// lock is held" is a borrow-checker fact at the call site.
pub struct LockToken { file: std::fs::File, path: PathBuf }
#[derive(Serialize, Deserialize)]
pub struct LockOwner { pub pid: u32, pub host: String, pub cmd: String, pub at: DateTime<Utc> }
impl LockToken {
    /// LOCK_EX|LOCK_NB, 25ms poll to `timeout` (cfg.lock_timeout_secs, default 5s).
    /// The holder note is written AFTER acquiring, so a reader may legitimately
    /// see it empty -> render "held by an unknown process", never a wrong pid.
    pub fn acquire(layout: &Layout, owner: LockOwner, timeout: Duration) -> Result<LockToken>;
}
impl Drop for LockToken { /* truncate note, LOCK_UN */ }

// ── store.rs ─────────────────────────────────────────────────────────────────
/// Glob-and-parse. ~55ms at 2000 tickets (measured). Loads OPEN proposals only;
/// walks `proposals/closed/` for IDS ONLY, never bodies.
pub fn load_snapshot(ctx: &Ctx) -> Result<Snapshot>;

pub struct Committed { pub snapshot: Snapshot, pub touched: Vec<PathBuf>,
                       pub minted: Vec<EntityRef>, pub rev: u64 }

pub struct Store<'c> { ctx: &'c Ctx }
impl<'c> Store<'c> {
    pub fn open(ctx: &'c Ctx) -> Store<'c>;

    /// THE single write path. CLI handlers and axum POST handlers call it
    /// byte-identically, because both go through the same `cmd::*` function.
    ///
    ///  1. `LockToken::acquire` (typed, diagnosable contention error)
    ///  2. load a FRESH Snapshot *inside* the lock — never trust one taken
    ///     before we had exclusivity ✅
    ///  3. build a `Minter` over `snap.taken_ids()` (incl. `closed_ids`)
    ///  4. run the PURE planner; all validation lives there
    ///  5. `Plan::validate`
    ///  6. `fm::writable()` on every frontmatter the plan touches, BEFORE any
    ///     byte moves — turns "silently appended a duplicate key" into a typed
    ///     refusal; `SetOutcome::ReplacedMultiline` is a HARD error
    ///  7. apply: stage per file, then tmp + `fs::rename` + fsync the dir. A
    ///     ticket's frontmatter delta and its `## Log` line are ONE write to ONE
    ///     file; cross-file plans order the authoritative file LAST
    ///  8. `transitions::prove()` on every touched ticket — the write path proves
    ///     its own legality, so a hand-edit is caught by the very NEXT VERB
    ///     instead of by CI weeks later ✅ (best enforcement timing in the field)
    ///  9. `sync = "commit"` AND the plan wrote something git tracks
    ///     (`plan.ops.iter().any(Op::writes_tracked_file)`, §2.10)
    ///     -> `git add -A .kanspec && git commit -m "kanspec: <verb> <id>"`
    /// 10. drop the lock; return the post-write snapshot with `rev + 1`
    ///
    /// ROUND-B CORRECTION to step 9's condition. A `scan`'s ENTIRE plan is one
    /// `Op::WriteGitState` against the gitignored cache, and committing for it
    /// (a) ran `git add -A -- .kanspec/**`, sweeping whatever tracker edits were
    /// pending — the normal resting state under the `sync = "batch"` default —
    /// into a commit labelled after the scan, and (b) had the `post-merge` hook's
    /// `kanspec scan --quiet` reach for git's index in the middle of a merge.
    /// Pinned by `scan_ladder.rs::a_scan_commits_nothing_under_sync_commit_…`.
    ///
    /// ROUND-C CONTRACT CHANGE (granted; requested independently by S3, S5, S6).
    /// `verb` is a TICKET transition verb and reaches ONLY that commit subject.
    /// It is `Some(v)` when the transaction IS ticket verb `v` — every
    /// `Op::Transition`, plus `new`'s genesis `CreateEntity` — and `None` when no
    /// ticket verb happened: a cache write, a projection rewrite, `doctor --fix`,
    /// and every knowledge verb (`spec new`, `decide`, `accept`, `supersede`,
    /// `revoke`, `quirk add|fix`, `features --confirm`). `None` commits as
    /// `kanspec: update <id>`.
    ///
    /// Round B left this as a bare `Verb` on the grounds that the value reached
    /// nothing. It did: TEN of the crate's twenty call sites passed
    /// `Verb::Confirm` as filler, so under `sync = "commit"` a `spec new auth`
    /// was committed as `kanspec: confirm auth`. `Confirm` means one specific
    /// thing — the human merge override (D-11) — and borrowing it as a filler
    /// made git history state something false about who attested to what.
    ///
    /// gh/network calls MUST happen BEFORE `transact` (see `Facts`, §2.16): a
    /// wedged subprocess inside the lock stalls the browser and every CLI verb.
    pub fn transact<F>(&self, verb: Option<Verb>, cmdline: &str, planner: F)
        -> Result<Committed>
    where F: FnOnce(&Snapshot, &Minter) -> Result<Plan>;
}

/// The ONLY fs-mutating functions in the crate. `tests/single_write_path.rs`
/// greps every other source file for `fs::write|fs::rename|File::create|
/// OpenOptions|fs::remove|fs::create_dir` and fails on any hit outside this file.
/// The allowlist is exactly TWO files and its own length is asserted, so it
/// cannot grow silently: `lock.rs` (creates the very lockfile it then locks) and
/// `cmd/init.rs` (scaffolds `.kanspec/` before a store can exist).
///
/// ROUND-A CORRECTION. This used to say `hooks.rs` / `setup.rs` "write outside
/// `.kanspec/`" and are exempted by a second grep. They are NOT exempt — the grep
/// skips only `store.rs` and the allowlist — so those two files may not contain a
/// mutator at all. That turned out to be the better design and it stands: `hooks.rs`
/// and `setup.rs` are PURE PLANNERS over a typed `hooks::Edit`, and the allowlisted
/// `cmd::init::apply` is their single applier — the same planner/applier split
/// `Store::transact` makes for the store. The second grep still runs, and now asserts
/// something stronger than it was written for: those two planners may never name a
/// path under `.kanspec/`, so the store's ground stays `store.rs`'s alone.
pub(crate) fn write_atomic(p: &Path, bytes: &[u8], _t: &LockToken) -> Result<()>;
pub(crate) fn append_line(p: &Path, line: &str, _t: &LockToken) -> Result<()>;
pub(crate) fn move_dir(a: &Path, b: &Path, _t: &LockToken) -> Result<()>;
pub(crate) fn create_new(p: &Path, bytes: &[u8], _t: &LockToken) -> Result<()>;
```

### 2.14 `src/fm.rs` — the surgical frontmatter writer

Body is the recon module **verbatim** (365 lines, compiled and tested; port from
`.../scratchpad/fmtest/src/fm.rs`). `gray_matter` is **deleted from DESIGN.md's crate list**
(cannot serialize at all, and mutates the content it hands back — no byte-identical write path can
exist on top of it). `yaml-edit` is rejected (silently truncates on read, welds lines on write).
`serde_yaml_ng::to_string` is never called: a no-op round trip reformats 11 of 22 lines.

```rust
#[derive(Debug)] pub enum FmError { NoFrontmatter, Unterminated }
/// `open + fm + close + body` reconstructs the input byte-exactly.
#[derive(Debug, Clone)] pub struct MdDoc { pub open: String, pub fm: String,
                                           pub close: String, pub body: String }
impl MdDoc { pub fn render(&self) -> String; }
pub fn split(src: &str) -> std::result::Result<MdDoc, FmError>;

#[derive(Debug, Clone)] pub struct KeySpan { pub key: String, pub line: usize,
    pub block_end: usize, pub val: (usize, usize), pub multiline: bool }
pub fn index(fm: &str) -> Vec<KeySpan>;                    // CRLF- and quote-aware

#[derive(Debug, Clone, PartialEq)]
pub enum Yv { Null, Bool(bool), Int(i64), Str(String), List(Vec<Yv>),
              /// ROUND-C ADDITION (granted, S6). A one-line FLOW mapping —
              /// `{sha: a1b9c3d, at: 2026-08-31T12:00:00Z, by: dev, why: …}`.
              /// `spec.stale_ack` is the one v0.1 field whose value is a struct:
              /// `SpecKey::StaleAck` and `model::StaleAck` were both already in
              /// this contract with no `Yv` able to express them. Verified
              /// empirically against serde_yaml_ng — the flow-map form
              /// deserializes into `StaleAck`; a flow SEQUENCE is rejected; and
              /// `Yv::Str("{…}")` is single-quoted by `emit`'s first-byte check
              /// (`{` is in `plain_ok`'s deny list) so it reads back as a String,
              /// which makes the WHOLE SPEC fail to load and takes `rules`,
              /// `prime` and the feature map down with it.
              /// FLOW style, never a block map, is load-bearing: it keeps the
              /// value on ONE line, so `index` reports `multiline: false` and a
              /// second `features --confirm` is an ordinary `SetOutcome::Replaced`
              /// rather than R-9's hard refusal.
              Map(Vec<(String, Yv)>) }
impl Yv {
    pub fn s(v: impl Into<String>) -> Yv;
    pub fn opt_s(v: Option<impl Into<String>>) -> Yv;      // None -> Yv::Null
    pub fn list(v: impl IntoIterator<Item = String>) -> Yv;
}
pub fn emit(v: &Yv, flow: bool) -> String;

#[derive(Debug, PartialEq)]
pub enum SetOutcome { Unchanged, Replaced, ReplacedMultiline, Inserted }
/// Replaces ONLY the value's byte range. Inline comments, key order, quoting
/// style, block scalars, unknown future keys and CRLF all survive. `ship`
/// produces a 3-line real git diff; 100 round-tripping edits reproduce the file
/// byte-identically.
pub fn set(doc: &mut MdDoc, key: &str, val: &Yv, order: &[&str]) -> SetOutcome;
pub fn append_to_section(doc: &mut MdDoc, heading: &str, line: &str);
pub fn mark_step(doc: &mut MdDoc, index: usize, done: bool) -> bool;
/// SAFETY GUARD — `Store::transact` calls this before ANY byte moves, and
/// `doctor::check_frontmatter_writable` calls it on every file. It compares the
/// line indexer's top-level keys against serde_yaml_ng's; a mismatch (quoted
/// key, explicit `?` key, key with spaces) becomes a typed refusal instead of a
/// silently appended duplicate key.
pub fn writable(fm_text: &str) -> std::result::Result<(), String>;
```

`ReplacedMultiline` is a **hard `KsError::Invalid`** in `store.rs`, never a silent collapse. No
v0.1 or v0.2 field holds a block scalar or nested map; if the schema ever grows one, this is where
it fails loudly.

### 2.15 `src/scan.rs` — the sealed proof and the ladder

```rust
/// Private fields; NO Default, NO Deserialize, NO From<MergeFact>. The only
/// constructors live in THIS file and each ran a real ladder. `plan_done`'s
/// signature therefore makes invariant 1 a COMPILE-TIME guarantee: a `done`
/// that never consulted git does not build. Same-module privacy — no
/// `pub(in ...)`, so it compiles (§2.7). ✅
#[derive(Clone, Debug, Serialize)]
pub struct MergedProof { ticket: TicketId, sha: Sha, method: Method,
                         pr: Option<u64>, checked_at: DateTime<Utc> }
impl MergedProof {
    pub fn ticket(&self) -> &TicketId;  pub fn sha(&self) -> &Sha;
    pub fn method(&self) -> Method;     pub fn pr(&self) -> Option<u64>;
    pub fn checked_at(&self) -> DateTime<Utc>;
    /// "IN MAIN (gh-pr #142 · checked 11s ago)". ROUND-B CORRECTION: takes the
    /// CALLER'S clock (`ctx.now`), never `Utc::now()`. Determinism comes from
    /// exactly three env overrides (§9), and a badge reading the wall clock
    /// renders "checked 3h ago" under `KANSPEC_NOW` — which would make every
    /// snapshot test of `done`'s transcript unstable. `derive::Badge::text`
    /// already took `now` for the same reason; these two now agree.
    pub fn badge(&self, now: DateTime<Utc>) -> String;
}

/// The chore/docs escape — a DIFFERENT type, so the landed path cannot accept
/// it, and `record` needs `&mut Plan` so the waiver is DURABLE before it is
/// usable. `plan_done` additionally refuses it from `review`, so `--no-code`
/// provably cannot bypass the gate on an already-shipped ticket. ✅
#[derive(Clone, Debug, Serialize)]
pub struct NoCodeWaiver { why: String, by: String, at: DateTime<Utc> }
impl NoCodeWaiver {
    /// Refuses an empty `why`. The durable op it pushes is a PROSE line under the
    /// ticket's `## Log` (`  no-code waiver by <actor> at <ts>: <why>`),
    /// deliberately shaped so `logentry::parse_log` skips it and `replay` is
    /// unaffected — a second PARSEABLE entry for one act would break the very
    /// proof the log exists for, and there is no `no_code` `TicketKey` (invariant
    /// 1). Pinned by a unit test asserting the line does NOT parse as a log entry.
    ///
    /// S5 NOTE: `record` needs `&mut Plan` while `DoneFacts.landed` is built
    /// BEFORE `plan_done` runs, so call it from INSIDE `plan_done` (it is pure —
    /// no IO), not from the handler.
    pub fn record(plan: &mut Plan, id: &TicketId, why: &str, by: &Actor, at: DateTime<Utc>)
        -> Result<NoCodeWaiver>;         // refuses an empty `why`
    pub fn why(&self) -> &str;
    pub fn by(&self) -> &str;
    pub fn at(&self) -> DateTime<Utc>;
}
#[derive(Clone, Debug, Serialize)] pub enum Landed { Proof(MergedProof), NoCode(NoCodeWaiver) }

/// Minted ONLY by `scan_all`; required by `Op::WriteGitState`. ✅
pub struct ScanToken(());

/// Three-state per rung. Ancestry-NEGATIVE is Inconclusive, not NotMerged: a
/// squash-merged branch is genuinely not an ancestor of main. Only rungs that
/// can PROVE absence may say No. Sharpest statement of invariant 2 in the field.
pub enum Rung { Merged(Evidence), NotMerged(Evidence), Inconclusive(Unknown) }
#[derive(Clone, Debug, Serialize)]
pub struct Evidence { pub method: Method, pub saw: String, pub sha: Option<Sha>, pub pr: Option<u64> }

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict { Landed { sha: Sha, method: Method, pr: Option<u64> },
                   NotLanded, Unknown(Unknown) }

/// A completed ladder run. Carries its own `rungs`, so `scan --explain` is a
/// property of the value `detect` already produced — there is no SECOND ladder
/// run that could disagree with the first. ✅ (Skeleton-First had a separate
/// `explain()` that re-runs.)
#[derive(Clone, Debug, Serialize)]
pub struct Detection { verdict: Verdict, checked_at: DateTime<Utc>,
                       fetch_age_secs: Option<u64>, rungs: Vec<RungTrace> }
impl Detection {
    fn seal(v: Verdict, at: DateTime<Utc>, age: Option<u64>, r: Vec<RungTrace>) -> Detection;
    pub fn verdict(&self) -> &Verdict;
    pub fn checked_at(&self) -> DateTime<Utc>;
    pub fn explain(&self) -> &[RungTrace];
    /// Down-converts the sealed, in-process value to the plain cache DTO. The
    /// cache is badge-grade; the gate is proof-grade. There is deliberately no
    /// inverse. ✅ (Sealed Keel's `GitState: Deserialize` holding a
    /// Serialize-only `Detection` does not compile — verified E0277 — and the
    /// obvious repair silently converts the cache into a forgery channel.)
    pub fn to_fact(&self, changed: Vec<String>) -> MergeFact;
}

/// THE LADDER, in the recon-corrected order. Every rung returns `Rung`; exit 128
/// anywhere is Unknown, never No.
///
///  0.  guard  rev-parse --verify --quiet '<head>^{commit}'
///                                          -> Unknown::HeadNotInObjectStore
///  0b. guard  ONLY when the SHA came from a live branch tip with no recorded
///             `head:` — rev-list --count <main>..<head> == 0
///                                          -> Unknown::ZeroCommitBranch
///             (a fresh `start` branch is trivially an ancestor of main —
///              a VERIFIED false MERGED for work that never happened. The
///              provenance condition is round B's correction: see §7, and the ⚠
///              on `StartFacts` in §2.16 for what would re-break it.)
///  1.  ANCESTRY  merge-base --is-ancestor <head> <main>   0=Merged 1=next 128=Unknown
///  2.  GH        pr view <n> | pr list --head <branch>; MERGED -> RE-VERIFY with
///                is-ancestor(mergeCommit.oid); mismatch -> Unknown::GhMergedButNotAncestor;
///                any gh failure -> Unknown::GhUnavailable, NEVER NotMerged
///  3.  TRAILER   log <main> -E --grep 'Kanspec: t-9c41([^0-9a-f]|$)' --format=%H
///                UNANCHORED (git indents squash-body trailers 4 spaces) and
///                boundary-terminated (ids are 4 hex; `t-9c4` would match `t-9c41`).
///                Blind to reverts -> weighted BELOW ancestry.
///  4.  PATCH-ID  cherry <main> <head> — RELABELLED "rebase/cherry-pick detection".
///                DESIGN.md rung 4 is backwards: recon measured a real 2-commit
///                squash as `+2`, i.e. NOT merged — the exact case the rung was
///                added for. Merged iff output non-empty AND every line is `-`.
///                Any `+` -> Unknown::SquashSuspectedNoGh.
///  5.  otherwise Unknown, with every RungTrace attached.
pub fn ladder(git: &Git, gh: &Gh, t: &Ticket, main: &str,
              fetch_age: Option<Duration>, now: DateTime<Utc>) -> Detection;

/// The `done` gate. RE-RUNS the ladder rather than trusting the cache — "a 60s-old
/// merged is not a gate". ✅ (One Gate reads `MergeVerdict` straight out of
/// gitstate.json.)
pub fn proof_for_done(ctx: &Ctx, t: &Ticket) -> Result<MergedProof>;

/// Runs the ladder across every non-terminal ticket + spec anchors + branch facts.
/// The ONLY producer of `ScanToken`. Runs OUTSIDE the lock (gh/network); the
/// caller then opens a short `transact` to persist via `Op::WriteGitState`.
pub fn scan_all(ctx: &Ctx, snap: &Snapshot, opts: ScanOpts) -> Result<(GitState, ScanToken)>;
pub struct ScanOpts { pub fetch: bool, pub only: Option<TicketId>, pub quiet: bool }

/// ROUND-B ADDITION. `--explain` needs the `Detection` behind each fact and the
/// cache DTO cannot carry a trace, so the sealed runs come back BESIDE the DTO
/// rather than being re-derived — there is still exactly one ladder run per
/// ticket per scan. `scan_all` delegates and keeps its signature above, which is
/// what `server.rs` calls.
pub type ScanOutcome = (GitState, ScanToken, Vec<(TicketId, Detection)>);
pub fn scan_all_detailed(ctx: &Ctx, snap: &Snapshot, opts: ScanOpts) -> Result<ScanOutcome>;

/// ROUND-B ADDITION, public because `done` needs exactly this for
/// `DoneFacts.touched` (§2.16). `Git::changed_paths` is three-dot only, so it
/// reports NOTHING for a branch that landed as a true merge (merge-base(main,
/// head) is then the head itself); this diffs each trailer-matched commit on main
/// to recover the paths. A second caller reaching for `Git::changed_paths`
/// directly silently reacquires that hole.
pub fn touched_paths(git: &Git, t: &Ticket, main: &str) -> Vec<String>;

/// The recorded human override. Appends an attributed `Verb::Confirm` line to the
/// TICKET'S `## Log`, not a cache entry: a human attestation is an ASSERTED ACT
/// WITH AN ACTOR, so it must survive `rm -rf cache/` and be visibly signed. ✅
pub fn plan_confirm(snap: &Snapshot, f: &ConfirmFacts, id: &TicketId) -> Result<Plan>;
pub struct ConfirmFacts { pub sha: Option<Sha>, pub actor: Actor,
                          pub at: DateTime<Utc>, pub why: String, pub invocation: String }
/// Reads a recorded confirmation back out of the log — the ONLY non-ladder route
/// to a `MergedProof`.
///
/// ROUND-B CORRECTION — takes `&Git`. The original `(t: &Ticket)` is
/// UNIMPLEMENTABLE: `Sha`'s only constructor is private to `git.rs`, so a
/// function with no `&Git` cannot fill `MergedProof.sha`. `&Git` is also the
/// stronger seal — the attested commit is re-resolved through git, so an
/// attestation naming a commit this repo does not have yields no proof at all.
pub fn confirmed_proof(git: &Git, t: &Ticket) -> Option<MergedProof>;
```

**Where the confirmation actually lives, end to end (D-11).** `plan_confirm` writes one
attributed `Verb::Confirm` line to the ticket's `## Log` whose NOTE IS THE STORAGE —
`in main <sha> — <why>` — and every later `scan_all` reads it back out of the log and
projects it into a fresh cache as a `Method::HumanConfirm` fact. That projection is what
makes the override survive `rm -rf .kanspec/cache` rather than needing to be repeated, and
`proof_for_done` consults it only AFTER the ladder declines, so an attestation can never
overrule fresh git truth.

### 2.16 The planner shape — pure, with `Facts`

```rust
/// Everything a planner needs from the outside world, gathered BEFORE `transact`.
/// The planner is GENUINELY pure: no `Ctx`, no git, no gh, no clock, no fs.
/// ✅ One Gate's `Planner<A> = fn(&Snapshot, &A, &Ctx, &Minter)` handed every
/// planner Git/Gh/Ui/clock and its own worked example shelled out to git inside
/// the lock, falsifying the purity claim for exactly `ship` and `done`.
pub struct Facts { pub actor: Actor, pub at: DateTime<Utc>, pub invocation: String }
/// ⚠ **`plan_start` must NOT write `TicketKey::Head`.** `StartFacts.head` is the
/// branch's fork point, for the `## Log` note and the branch fact — not for the
/// frontmatter. `head:` is written by `ship`/`done` out of real git output, and
/// guard 0b (§7) keys off exactly that: a SHA from a live branch tip with no
/// recorded `head:` is how the ladder recognises a branch that never carried a
/// commit. Set `head:` at claim time and every freshly-started ticket reads back
/// as MERGED by ancestry — a VERIFIED false positive for work that never
/// happened, which is the single worst answer this tool can give.
pub struct StartFacts { pub base: Facts, pub branch: String,
                        pub worktree: Option<PathBuf>, pub head: HeadSha }
pub struct ShipFacts  { pub base: Facts, pub head: HeadSha }
pub struct DoneFacts  { pub base: Facts, pub landed: Landed, pub touched: Vec<ChangedPath> }

/// The uniform planner shape. Every mutating verb is one of these, and each is a
/// table-driven unit test against a hand-built `Snapshot` with ZERO IO.
/// (Signatures below are exhaustive for v0.1; V2 adds its own in its own files.)
pub fn plan_new   (s:&Snapshot, f:&Facts,      a:&NewArgs,   m:&Minter) -> Result<Plan>;
pub fn plan_start (s:&Snapshot, f:&StartFacts, a:&StartArgs, m:&Minter) -> Result<Plan>;
pub fn plan_ship  (s:&Snapshot, f:&ShipFacts,  a:&ShipArgs,  m:&Minter) -> Result<Plan>;
pub fn plan_done  (s:&Snapshot, f:&DoneFacts,  t:&Triage,    a:&DoneArgs, m:&Minter) -> Result<Plan>;
pub fn plan_park  (s:&Snapshot, f:&Facts,      a:&ParkArgs,  m:&Minter) -> Result<Plan>;
pub fn plan_drop  (s:&Snapshot, f:&Facts,      a:&DropArgs,  m:&Minter) -> Result<Plan>;
pub fn plan_repair(s:&Snapshot, f:&Facts,      a:&RepairArgs,m:&Minter) -> Result<Plan>;
```

Worked example — **`ship`, both call sites, byte-identical:**

```rust
// src/cmd/flow.rs
pub fn ship(ctx: &Ctx, a: &ShipArgs) -> Result<ShipReport> {
    let snap = ctx.snapshot()?;                       // read-only peek, outside the lock
    let t = snap.ticket(&TicketId::parse(&a.id)?)?;
    // Every subprocess happens HERE, before the lock is taken.
    let head = ctx.git.head_sha(t.fm.branch.as_deref().unwrap_or("HEAD"))?;
    let f = ShipFacts { base: Facts { actor: ctx.actor.clone(), at: ctx.now,
                                      invocation: ctx.invocation() }, head };
    let done = Store::open(ctx).transact(Some(Verb::Ship), &ctx.invocation(),
                                         |s, m| plan_ship(s, &f, a, m))?;
    Ok(ShipReport::from(&done, a))
}

// src/server.rs — the SAME function. There is no server-side write code at all.
//
// ROUND-D CORRECTION (D-40): the anchor is cloned, but a FRESH `Ctx` is built per
// request. Reusing `st.ctx` would freeze `Ctx::now` at server start and stamp that
// time into the `## Log` of every ticket a POST moved — which `replay`'s monotonicity
// check turns into a permanently unwritable ticket as soon as a CLI verb in another
// terminal has logged a later time.
async fn post_verb(State(st): State<AppState>, Json(a): Json<ShipArgs>)
    -> std::result::Result<Json<ShipReport>, ApiError> {
    let anchor = st.ctx.clone();                                  // Arc<Ctx>: Send + Sync ✅
    Ok(Json(tokio::task::spawn_blocking(move || {
        cmd::flow::ship(&server::request_ctx(&anchor)?, &a)       // fresh clock, same primary
    }).await??))
}
```

### 2.17 `src/triage.rs` — one typed value, two front doors

```rust
/// The interactive prompts and the `--json` flags BOTH construct this, and only
/// this reaches `plan_done`. The agent path and the human path therefore cannot
/// diverge in what they record.
pub struct Triage { pub steps: Vec<StepDisposition>, pub quirks: Vec<NewQuirk>,
                    pub decisions: Vec<NewDecision>, pub spec: SpecCheck }
pub enum StepDisposition { Spawn { index: usize, title: String },
                           Drop  { index: usize, why: String },
                           ActuallyDone { index: usize } }
pub enum SpecCheck { EditedOnBranch { specs: Vec<SpecName> },
                     Unchanged { why: String }, NotApplicable }
pub struct NewQuirk    { pub title: String, pub paths: Vec<String>, pub severity: Severity }
pub struct NewDecision { pub title: String, pub scope: Vec<String> }

impl Triage {
    /// Non-interactive REFUSES with a typed error naming BOTH flags when neither
    /// `--spawn` nor `--no-followups` is present — clap cannot express "required
    /// iff --json" (`required_if_eq` works on values, and global+required is the
    /// debug-only panic the recon found), and the handler yields a better message.
    pub fn from_args(t: &Ticket, a: &DoneArgs, touched: &[ChangedPath], s: &Snapshot)
        -> Result<Triage>;
    pub fn prompt(t: &Ticket, a: &DoneArgs, touched: &[ChangedPath], s: &Snapshot)
        -> Result<Triage>;
}
```

---

## 3. Error strategy

**Shape, not situation.** Eight closed shapes (§2.1). A new refusal is
`KsError::gate(GateCode::Undispositioned, msg, fixes![..])` **in the raising agent's own file** —
`error.rs` never grows, which removes the single worst merge magnet from a 9-agent build. `code:
&'static str` remains a stable JSON discriminator, so agent-facing error kinds stay as precise as
a per-situation enum. The two errors whose output quality *is* the product keep structured
payloads via `GateDetail` (`NotLanded { trace }`, `Undispositioned { items }`) — pre-formatting a
ladder trace into a `message` string would throw away the `--explain`-grade output that is this
tool's selling point.

**Invariant 9 is a type constraint.** `Fixes(Fix, Vec<Fix>)` is non-empty by construction, so you
cannot build an error without naming the next command, and unlike a `debug_assert` it survives
`cargo build --release`. `KsError::fixes()` is **total** — including `Internal`, which gets
`kanspec doctor`. There is **no `#[from] std::io::Error`**: in a file-munger that conversion would
make every bare `?` emit a fix-less error, i.e. invariant 9 opt-out-by-default.

**A typed error renders its exact next command:**

```
✗ t-9c41 is not on origin/main — cannot close it
    ancestry   merge-base --is-ancestor a1b9c3d origin/main   exit 1   not an ancestor
    gh-pr      (gh unavailable: HTTP 401 Bad credentials)     exit 1   inconclusive
    trailer    log origin/main --grep 'Kanspec: t-9c41…'      exit 0   0 hits
    patch-id   cherry origin/main a1b9c3d                     exit 0   +2  squash suspected
  unknown (squash suspected, no gh)
  → kanspec scan --explain t-9c41
  → kanspec scan --confirm t-9c41 --why "..."
```

```json
{"ok":false,"error":{"kind":"gate","code":"merge_unknown",
 "message":"cannot verify t-9c41 landed",
 "detail":{"detail":"not_landed","trace":[{"method":"ancestry","exit":1,...}]},
 "fix":["kanspec scan --explain t-9c41","kanspec scan --confirm t-9c41 --why \"...\""],
 "exit":1}}
```

**Exit codes.** `0` ok · `1` gate refusal / invariant violation / `doctor` findings · `2`
**landcheck Stop-hook block ONLY** · `64` usage · `69` environment · `70` internal.

`main` uses `try_get_matches()` + `from_arg_matches_mut` and returns `ExitCode`; it **never** calls
`Cli::parse()` (which internally `process::exit(2)`s on any typo'd flag) and never
`process::exit`. Clap's native 2 is remapped to 64. `2` is reachable from exactly one module,
sealed by `BlockToken`'s private field, so the Stop-hook contract is auditable by grep.
`dispatch` returns `Result<u8, KsError>` so a **successful** run can still exit non-zero (`doctor`
→ 1, `landcheck` → 2) without going through the error renderer.

```rust
// src/lib.rs — frozen after wave 0
pub fn run(invoked_as: &'static str) -> std::process::ExitCode {
    let cmd = Cli::command().name(invoked_as).bin_name(invoked_as);
    let cli = match cmd.try_get_matches().and_then(|mut m| Cli::from_arg_matches_mut(&mut m)) {
        Ok(c)  => c,
        Err(e) => { let _ = e.print(); return ExitCode::from(match e.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => code::OK,
            // bare `kanspec` / bare `kanspec comment` land here; USAGE, not OK, so an
            // agent that runs an incomplete command does not think it succeeded (§11 D-16)
            _ => code::USAGE }); }
    };
    let color = out::apply_color_policy(cli.color);
    let mode  = if cli.json { OutMode::Json } else { OutMode::Human { color } };
    let ctx = match Ctx::open(&cli, &cwd()) {
        Ok(c) => c, Err(e) => { e.render(&mode); return ExitCode::from(e.exit_code()) } };
    match dispatch(&ctx, &cli) {
        Ok(c)  => ExitCode::from(c),
        Err(e) => { e.render(&ctx.out); ExitCode::from(e.exit_code()) }
    }
}
```

---

## 4. The Store / single-write-path, locking, surgical writes

Covered as code in §2.13–2.14. The load-bearing points:

| Concern | Mechanism | Why not the alternative |
|---|---|---|
| One write path | `store::{write_atomic,append_line,move_dir,create_new}` are `pub(crate)` and each takes `&LockToken`; `Store::transact` is the only public mutator | Module privacy gets 90%; Rust cannot forbid `std::fs` crate-wide, so the last 10% is `tests/single_write_path.rs` — named after the invariant, not pretended away |
| Lock | **flock(2)** held for the transaction's whole life; released by the kernel on process death | No stale reaper to get wrong (pid reuse, clock skew); `kill -9` cannot wedge the repo |
| Lock diagnostics | `LockOwner` JSON written **after** acquiring; empty note renders "held by an unknown process" | flock alone gives a blocked syscall and nothing to print, but invariant 9 demands a fix line |
| Staleness of reads | Snapshot loaded **inside** the lock | A snapshot taken before exclusivity is a TOCTOU |
| Atomicity | tmp + `fs::rename` + dir fsync, per file; state delta and Log line are ONE write to ONE file; cross-file plans order the authoritative file **last** | Multi-file plans are genuinely not atomic — see §11 R-1; `doctor` names the half-applied state |
| Frontmatter | hand-rolled surgical writer; `serde_yaml_ng` read-only | `gray_matter` cannot serialize and mutates content; `yaml-edit` truncates on read; full re-serialize changes 11/22 lines |
| Pre-write guard | `fm::writable()` on every touched file, inside the lock, before any byte moves | Converts the indexer's one real hazard (non-plain keys → duplicate key appended) into a typed refusal |
| Post-write proof | `transitions::prove()` on every touched ticket | A hand-edited `state:` fails at the **next verb**, not at the next CI run |
| Derived cache | `Op::WriteGitState { token: ScanToken, .. }` — inside the lock, capability-gated | An unlocked `cache.rs` writer lets the server's 60s poll and a post-merge hook interleave on the sole home of every derived fact |
| gh / network | **must** run before `transact` | A wedged `gh` inside the lock stalls the browser and every concurrent CLI verb for the lock timeout |

**ID minting.** Inside `transact`, under the lock, against the fresh snapshot's `taken_ids()`
(which includes `closed_ids`), retrying on collision, widening 4→5 hex after 64 rejections.
Collision-freedom is a property of **exclusion**, not hash entropy — 16 bits is 65536, so birthday
collisions bite near ~300 entities. It works across parallel agents and worktrees precisely
because worktree unification means they all contend on ONE lock over ONE primary `.kanspec/`. The
`t-`/`p-`/`D-`/`q-` prefixes are load-bearing twice: type safety, and stopping a bare 4-hex id like
`0x1f` from being reinterpreted as a YAML number.

---

## 5. `rules ≡ prime` — one generator, one renderer (invariant 3)

```rust
// src/rulesdoc.rs
pub struct Scope { pub paths: Vec<String>, set: GlobSet }   // empty = unscoped
impl Scope {
    pub fn none() -> Scope;
    pub fn of(paths: &[String]) -> Result<Scope>;
    pub fn from_branch(touched: &[ChangedPath]) -> Result<Scope>;
    pub fn matches(&self, p: &str) -> bool;
}

#[derive(Serialize)] pub struct RulesDoc {
    pub decisions:  Vec<StandingDecision>,   // ACCEPTED only; full text iff scope matches
    pub quirks:     Vec<StandingQuirk>,      // ACTIVE only, path-matched
    pub spec_rules: Vec<StandingRule>,       // with {p-xxxx} provenance tokens; rank order, within budget
    pub elided:     Vec<ElidedSpec>,         // matched specs past the budget — NAMED, never dropped
    pub counts:     Counts,
}
/// How strongly an entity's globs reach into a scope; `touches` is `rank(..).is_some()`.
/// Ordered: most specific glob (leading literal segments, then exact-path) first, then
/// the entity covering more of the scope's paths.
pub struct Rank { pub specificity: (usize, bool), pub paths: usize }
impl Scope { pub fn rank(&self, globs: &[String]) -> Option<Rank>; }

/// THE generator. `rules`, `rules --path`, and `prime` all call exactly this, under
/// `[prime] spec_budget_tokens` (0 = unlimited). Matched specs are shown in rank order
/// while the budget is unspent — soft, so the first spec always shows whole — and named
/// past it. PURE: `&Snapshot` cannot contain a closed proposal body, so invariant 4 is
/// enforced by what the input TYPE can hold.
pub fn build(s: &Snapshot, scope: &Scope) -> RulesDoc;
/// `rules --full`: the budget lifted. A separate entry point so `prime` cannot reach it.
pub fn build_full(s: &Snapshot, scope: &Scope) -> RulesDoc;

/// THE renderer — the ONLY way a RulesDoc becomes bytes. `kanspec rules` writes
/// exactly this and stops. `kanspec prime` writes exactly this, then "\n", then
/// the live slice. Byte-identity is a property of the CALL GRAPH: there is
/// physically no second formatter to drift from.
pub fn render_text(d: &RulesDoc) -> String;

pub fn audit(s: &Snapshot, d: &RulesDoc) -> Vec<AuditWarning>;   // rules --audit / --adopt
```

`cmd/prime.rs` produces the payload in **exactly one place**:

```rust
pub fn payload(ctx: &Ctx, s: &Snapshot, dv: &Derived, scope: &Scope) -> String {
    let standing = rulesdoc::render_text(&rulesdoc::build(s, scope));
    format!("{standing}\n{}", live_slice(ctx, s, dv))
}
```

**The test** (`tests/invariants_rules.rs`) combines both winning ideas: assert on the **real
binary's stdout** (catching a handler that adds a header or a trailing newline between the
generator and the terminal), **parameterized over scopes** (unscoped plus several `--path` values,
which is what makes the identity meaningful under path-scoped injection), plus
`prime --json .standing == rules --json .data`.

```rust
for scope in [vec![], vec!["src/auth/x.ts"], vec!["src/billing/**"], vec!["nonexistent/**"]] {
    let r = repo.ks(["rules"].iter().chain(path_flags(&scope))).stdout;
    let p = repo.ks(["prime"].iter().chain(path_flags(&scope))).stdout;
    assert!(p.starts_with(&r), "invariant 3 broke for scope {scope:?}");
}
```

---

## 6. Derived-state projection — `src/derive.rs`, pure

No `use std::fs`, no `use std::process`, no `Utc::now()` — `now` is a `Snapshot` field.
`tests/purity.rs` greps for all three and fails the build on a hit. This is what makes the part of
the product most likely to be wrong, and hardest to reproduce, testable as table-driven unit tests
over struct literals in microseconds. Every derived fact in the product is one of these:

```rust
// ── the dependency graph ─────────────────────────────────────────────────────
/// A dep is satisfied when it is TERMINAL (done or dropped) or IN-MAIN. Dropped
/// counts as satisfied: blocking forever on a dropped dep is worse, and
/// `doctor::check_orphan_deps` warns on a dep pointing at a dropped ticket.
pub fn dep_satisfied(s: &Snapshot, dep: &TicketId) -> bool;
pub fn is_ready(s: &Snapshot, t: &Ticket) -> bool;                 // todo && all deps satisfied
pub fn ready_queue(s: &Snapshot) -> Vec<&Ticket>;
/// ROUND-B CORRECTION — `t` borrows from the snapshot too (`&'s Ticket`). The
/// original `&Ticket` cannot return a dep that is MISSING from the snapshot, since
/// there is no `&'s TicketId` for a ticket that does not exist — and silently
/// dropping exactly the broken deps renders "blocked by nothing" for the case a
/// human most needs named. Every real call site takes its ticket out of the
/// snapshot, so no caller is affected.
pub fn blocked_by<'s>(s: &'s Snapshot, t: &'s Ticket) -> Vec<&'s TicketId>;
pub fn dep_cycles(s: &Snapshot) -> Vec<Vec<TicketId>>;

// ── the git overlay (reads ONLY s.git — the gitignored cache) ────────────────
pub fn merge_fact(s: &Snapshot, t: &Ticket) -> Option<&MergeFact>;
/// ROUND-B CORRECTION — NON-TERMINAL && Merged, wider than the original
/// `doing|review`. A ticket parked back to `todo` after its branch landed is in
/// the same "git says this shipped, nobody closed it" situation, and is the one
/// the tripwire most needs to catch. If S8's board wants Todo+merged in Backlog
/// rather than In-main, `derive::column` is the one line to change.
pub fn in_main(s: &Snapshot, t: &Ticket) -> Option<&MergeFact>;
pub fn badge(s: &Snapshot, t: &Ticket) -> Badge;
#[derive(Serialize)] pub enum Badge { Unpushed, Pushed, PrOpen { n: u64 },
    InMain { method: Method, sha: String, checked_at: DateTime<Utc> },
    Unknown { why: String, checked_at: Option<DateTime<Utc>> }, NeverScanned }

// ── the tripwires ────────────────────────────────────────────────────────────
pub fn stalled(s: &Snapshot, t: &Ticket)  -> Option<Duration>;     // doing, idle > stall_secs
pub fn dwell(s: &Snapshot, t: &Ticket)    -> Option<Tripwire>;
#[derive(Serialize)] pub enum Tripwire { ReviewDwell(Duration), InMainNotClosed(Duration),
                                         SettlingDwell(Duration), DiscoveredUntriaged(Duration) }
pub fn settling(s: &Snapshot, p: &Proposal) -> bool;               // every linked ticket terminal
pub fn unresolved(s: &Snapshot, p: &ProposalId) -> usize;          // v0.2
pub fn double_claims(s: &Snapshot) -> Vec<(TicketId, Vec<String>)>;

/// The attestation seam (D-12), both PURE and both read out of the `## Log` —
/// so they survive `rm -rf cache/`, travel through git, and never become a
/// frontmatter field (invariant 1).
///
/// `attested` is the standing attestation: the LAST `repair` entry, when the
/// state it attested is still the ticket's state — so an ordinary verb that
/// moves the ticket on SPENDS it. `badge` consults it first for a terminal
/// state and returns `Unknown { why: "attested done by …" }` rather than a
/// cache-derived merge badge, which is what stops a vouched-for close from
/// rendering like a proven one on ls / show / board / flow.
///
/// `logged_close` is the evidence `plan_repair` demands before it will attest a
/// `done`: a `done` entry (written only against a sealed `MergedProof` or a
/// recorded `--no-code` waiver) or a `confirm` entry (D-11, itself refused
/// without a SHA and a reason). Round-E fix: without it, a `sed` to
/// `state: done` plus `kanspec repair --why "trust me"` closed an unmerged
/// ticket and left `doctor` clean — a two-command laundering route strictly
/// MORE permissive than the `done --no-code` gate it bypassed.
pub fn attested(t: &Ticket) -> Option<&LogEntry>;
pub fn logged_close(t: &Ticket) -> bool;

/// Spec staleness, RECOMPUTED at read time from the git observations `scan`
/// recorded: `SpecAnchor::merges_since` — EVERY merge on main touching the spec's
/// `code:` globs since its last-edit anchor — widened by the per-ticket cached
/// `changed_paths`, which name examples. The two counts OVERLAP (a kanspec
/// ticket's merge is one of the merges git counted), so they combine with `max`,
/// NEVER a sum. NEVER an accumulated counter either: both are fresh answers to
/// git questions, so `rm -rf cache/` erases the answer (→ `NeverScanned`) rather
/// than resetting a tally to a confident zero, which would UNDER-fire the
/// tripwire — the dangerous direction. ✅
///
/// Round-E correction: this comment previously said "from per-ticket cached
/// `changed_paths`" and the code implemented exactly that, so the tripwire
/// counted only kanspec's OWN merged tickets and was silent on a teammate's PR,
/// a hotfix, a dependabot bump — anything predating adoption. `merges_since` was
/// written by `scan` and read by no production code at all.
///
/// A `stale_ack` that POSTDATES `last_edit_at` supersedes the recorded count
/// (the count is measured from the last edit and answers a question that ack has
/// closed); it is never subtracted, because a decrement is the counter D-10
/// forbids. An anchor with no point to count FROM is `NeverScanned`, never `Ok`.
pub fn staleness(s: &Snapshot, spec: &Spec) -> Staleness;
#[derive(Serialize)] pub enum Staleness {
    Ok,
    Stale { merges: u32, since: DateTime<Utc>, examples: Vec<TicketId> },
    DeadGlobs { globs: Vec<String> },
    NeverScanned,
}

// ── the board / status aggregates ────────────────────────────────────────────
pub fn column(s: &Snapshot, t: &Ticket) -> Column;
#[derive(Serialize)] pub enum Column { Backlog, Ready, Doing, Review, InMain, Done, Dropped }
/// THE status list: every attention line, grouped, each already carrying its fix.
pub fn attention(s: &Snapshot) -> Vec<Attention>;
#[derive(Serialize)] pub struct Attention { pub owner: Owner, pub glyph: char,
    pub subject: String, pub line: String, pub fix: String, pub url: Option<String> }
#[derive(Serialize)] pub enum Owner { You, Agent, Watching }

/// The one aggregate `status`, `board`, `up` and `prime` all read, so the
/// terminal, the browser and the agent cannot disagree — there is exactly one
/// implementation of "stalled".
pub fn compute(s: &Snapshot) -> Derived;
#[derive(Serialize)] pub struct Derived {
    pub ready: Vec<TicketId>, pub blocked: BTreeMap<TicketId, Vec<TicketId>>,
    pub in_main: BTreeMap<TicketId, Badge>, pub stalled: BTreeMap<TicketId, Duration>,
    pub settling: Vec<ProposalId>, pub dwell: BTreeMap<TicketId, Tripwire>,
    pub stale: BTreeMap<SpecName, Staleness>, pub attention: Vec<Attention>,
}
```

`derive` receives git facts through exactly one channel — `Snapshot.git`, the gitignored cache —
so "derived facts are never stored" reduces to "the projection's only git input is a disposable
file that cannot travel through git".

---

## 7. The git wrapper and the merge-detection ladder, as code

Wrapper API: §2.11. Universal invocation rules, each verified by recon: always `git -C
<primary_root>`; always scrub `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE` from the child env
(git sets `GIT_DIR` for hooks, and env beats `-C`); always `--path-format=absolute`; every
pathspec through `Pathspec` (`:(glob,top)`); never trust exit code alone for `log`/`cherry` (both
exit 0 on no matches); **exit 128 is always `Unknown`, never `No`**.

```rust
// src/scan.rs
pub fn ladder(git: &Git, gh: &Gh, t: &Ticket, main: &str,
              fetch_age: Option<Duration>, now: DateTime<Utc>) -> Detection {
    let mut tr: Vec<RungTrace> = Vec::new();
    let age = fetch_age.map(|d| d.as_secs());
    macro_rules! done { ($v:expr) => { return Detection::seal($v, now, age, tr) } }

    // ── guard 0: is there anything to ask about? ──────────────────────────────
    let Some(head) = head_of(git, t) else { done!(Verdict::Unknown(Unknown::NoHead)) };
    if !git.object_exists(&head) {
        tr.push(trace(Method::None, "rev-parse --verify --quiet", 1, "absent", "unknown"));
        done!(Verdict::Unknown(Unknown::HeadNotInObjectStore { sha: head.as_str().into() }))
    }
    // ── guard 0b: a branch that never carried a commit is not "merged" ───────
    //
    // ROUND-B CORRECTION, keyed off PROVENANCE rather than reachability. The
    // ordering above was unimplementable as written: after a TRUE merge the branch
    // tip is reachable from main, so `rev-list --count main..head` is 0 for a real
    // merge exactly as for a branch that never committed. Run before rung 1 the
    // guard answers `unknown` for the one shape ancestry can prove; run after a
    // NEGATIVE ancestry it can never fire at all, since count == 0 implies
    // ancestry. The two cases are genuinely indistinguishable by reachability, so
    // the guard fires only when the SHA came from a LIVE BRANCH TIP with no
    // recorded `head:` — and `head:` is written by `ship` out of real git output,
    // so its presence means the branch demonstrably carried work. See the ⚠ on
    // `StartFacts` (§2.16): `plan_start` writing `head:` would re-break this.
    if origin == HeadOrigin::BranchTip {
        match git.commits_ahead(main, &head) {
            Tri::Yes(0) => { tr.push(trace(Method::None, "rev-list --count", 0, "0", "unknown"));
                             done!(Verdict::Unknown(Unknown::ZeroCommitBranch)) }
            Tri::Unknown(u) => done!(Verdict::Unknown(u)),
            _ => {}
        }
    }

    // ── rung 1: ANCESTRY — exact for true merges and fast-forwards ────────────
    match git.is_ancestor(&head, main) {
        Tri::Yes(()) => { tr.push(trace(Method::Ancestry, "merge-base --is-ancestor", 0,
                                        "ancestor", "merged"));
                          done!(Verdict::Landed { sha: head, method: Method::Ancestry, pr: t.fm.pr }) }
        Tri::Unknown(u) => done!(Verdict::Unknown(u)),
        Tri::No => tr.push(trace(Method::Ancestry, "merge-base --is-ancestor", 1,
                                 "not an ancestor", "inconclusive")),
        // NOTE: ancestry-NEGATIVE is inconclusive, not NotMerged — a squash-merged
        // branch is genuinely not an ancestor. Only rungs that can PROVE absence say No.
    }

    // ── rung 2: GH — the ONLY rung that sees a title-only squash ──────────────
    if gh.available() {
        let prs = t.fm.pr.map(|n| gh.pr_view(n).map(|p| vec![p]))
                    .unwrap_or_else(|| t.fm.branch.as_deref()
                        .map(|b| gh.pr_for_head(b))
                        .unwrap_or(Ok(vec![])));
        match prs {
            Err(GhUnavailable(why)) => tr.push(trace(Method::GhPr, "gh pr", 1, &why, "inconclusive")),
            Ok(list) => if let Some(pr) = list.iter().find(|p| p.state == PrState::Merged) {
                if let Some(oid) = pr.merge_commit.as_deref().and_then(sha_of) {
                    // Re-verify gh's CLAIM as a local git FACT, and get a real SHA.
                    match git.is_ancestor(&oid, main) {
                        Tri::Yes(()) => { tr.push(trace(Method::GhPr, "gh + is-ancestor(mergeCommit)",
                                                        0, "MERGED", "merged"));
                            done!(Verdict::Landed { sha: oid, method: Method::GhPr,
                                                    pr: Some(pr.number) }) }
                        // stale fetch or a different base branch — NOT a confident answer
                        _ => done!(Verdict::Unknown(Unknown::GhMergedButNotAncestor {
                                 merge_sha: oid.as_str().into() })),
                    }
                }
            },
        }
    } else {
        tr.push(trace(Method::GhPr, "gh auth status", 1, "unavailable", "inconclusive"));
    }

    // ── rung 3: TRAILER — unanchored + boundary-terminated ────────────────────
    match git.grep_trailer(main, &t.fm.id) {
        Tri::Yes(shas) if !shas.is_empty() => {
            tr.push(trace(Method::Trailer, "log --grep", 0, &format!("{} hit(s)", shas.len()),
                          "merged"));
            // Blind to reverts, so it is weighted BELOW ancestry — the badge says so.
            done!(Verdict::Landed { sha: shas[0].clone(), method: Method::Trailer, pr: t.fm.pr })
        }
        Tri::Unknown(u) => done!(Verdict::Unknown(u)),
        _ => tr.push(trace(Method::Trailer, "log --grep", 0, "0 hits", "inconclusive")),
    }

    // ── rung 4: PATCH-ID — rebase/cherry-pick + SINGLE-commit squash only ─────
    // DESIGN.md rung 4 is backwards: recon measured a real 2-commit squash as
    // `+2`, i.e. NOT merged — the exact case the rung was supposed to cover.
    match git.cherry(main, &head) {
        Tri::Yes(lines) if !lines.is_empty() && lines.iter().all(|l| l.upstream) => {
            tr.push(trace(Method::PatchId, "cherry", 0, "all -", "merged"));
            done!(Verdict::Landed { sha: head, method: Method::PatchId, pr: t.fm.pr })
        }
        Tri::Yes(lines) => {
            let plus = lines.iter().filter(|l| !l.upstream).count();
            tr.push(trace(Method::PatchId, "cherry", 0, &format!("+{plus}"), "inconclusive"));
            // A `+` line CANNOT distinguish an unmerged branch from a multi-commit
            // squash, so it is Unknown — never NotMerged.
            done!(Verdict::Unknown(Unknown::SquashSuspectedNoGh { plus_lines: plus }))
        }
        Tri::Unknown(u) => done!(Verdict::Unknown(u)),
        Tri::No => {}
    }

    // Every rung declined. Stale fetch is the most actionable reason to name.
    if let Some(a) = age { if a > FETCH_MAX { done!(Verdict::Unknown(Unknown::FetchStale { age_secs: a })) } }
    done!(Verdict::NotLanded)
}
```

**The honest hole, verified by recon:** a multi-commit squash with a GitHub title-only merge
message and no `gh` defeats **all four rungs**. It renders `unknown (squash suspected, no gh)`
with the trace, and `kanspec scan --confirm <id> --why "..."` is the recorded human override —
which appends a `Verb::Confirm` line to the **ticket's `## Log`**, not to the cache, so the
attestation survives a cache wipe and is visibly signed.

---

## 8. `Cargo.toml` — in full

```toml
[package]
name        = "kanspec"
version     = "0.1.0"
edition     = "2021"                       # house style (~/dev/homerunner), NOT cargo's 2024 default
rust-version = "1.85"
description = "kanban + spec review over plain git-tracked files"
license     = "MIT"
default-run = "kanspec"

[[bin]]
name = "kanspec"
path = "src/bin/kanspec.rs"
[[bin]]
name = "ks"                                # cargo-dist ships every [[bin]]; a symlink does not survive
path = "src/bin/ks.rs"

[lib]
name = "kanspec"
path = "src/lib.rs"

[features]
default = []
# v0.2. Bundled rusqlite compiles SQLite from C on every clean build and is by far
# the largest compile cost in the set for code v0.1 never calls. Moves into
# `default` when ci.rs lands.
ci-homerunner = ["dep:rusqlite"]

[dependencies]
anyhow        = "1.0.104"
axum          = "0.8.9"                    # default features; SSE is NOT feature-gated
chrono        = { version = "0.4.45", features = ["serde"] }
clap          = { version = "4.6.6", features = ["derive"] }
clap_complete = "4.6.9"
comfy-table   = "8.0.0"
globset       = "0.4.20"
libc          = "0.2.189"                  # flock(2) only — ~40 lines, zero new supply chain
notify        = "8.2.0"
owo-colors    = { version = "4.4.0", features = ["supports-colors"] }
pulldown-cmark = { version = "0.13.4", default-features = false, features = ["html"] }
# `debug-embed` is NOT optional (added round A). Without it rust-embed reads `docs/`
# and `assets/` off disk in a debug build, at the absolute path baked in at compile
# time — so `kanspec instructions` lists no topics and 404s every real one, and `up`
# would serve the SPA only on its own build machine. Dogfooding runs debug builds.
rust-embed    = { version = "8.12.0", features = ["mime-guess", "debug-embed"] }
serde         = { version = "1.0.229", features = ["derive"] }
serde_json    = "1.0.151"
serde_yaml_ng = "0.10.0"                   # READ-SIDE DESERIALIZER ONLY; never to_string
thiserror     = "2.0.20"
tokio         = { version = "1.53.1", features = [
                  "rt-multi-thread", "macros", "signal", "time", "net", "fs", "sync"] }
tokio-stream  = { version = "0.1", features = ["sync"] }   # wrappers::BroadcastStream
toml          = "1.1.4"                    # NOTE: 1.x, not 0.8
rusqlite      = { version = "0.40.2", features = ["bundled"], optional = true }

# DELIBERATELY ABSENT:
#   gray_matter — cannot serialize at all, and mutates the content it returns
#   tower-http  — axum 0.8 alone covers SSE + a 12-line embedded-asset fallback;
#                 verified 9 direct deps / 1.3 MB stripped for the whole server
#   futures     — tokio-stream's merge + map_while replaces take_until
#   a git library — shell out for exact parity with the user's git

[dev-dependencies]
insta    = { version = "1.48.0", features = ["json", "filters"] }
tempfile = "3.27.0"

[profile.release]
lto    = true
strip  = true
opt-level = 3
codegen-units = 1
```

```rust
// build.rs — MANDATORY. rust-embed's include_bytes! tracks existing FILES but not
// the DIRECTORY, so a newly added asset is silently absent from a release binary
// (verified: build finished in 0.08s and the marker string was not in the binary).
fn main() { println!("cargo:rerun-if-changed=assets"); }
```

---

## 9. Test strategy

**Unit tests live inline** (`#[cfg(test)] mod tests`) **in the file under test**, owned by that
file's owner. No unit test ever lives in a shared file. **Integration tests are named after the
invariant they prove** so a failure reads as "invariant 3 broke", which is the review conversation
you want.

**The fast half (~1s, no git, no fs, no clock).** `derive.rs` against `Snapshot` literals;
`transitions::{next, replay}` over the exhaustive (State × Verb) matrix; `fm.rs` byte-stability +
100-edit idempotence against the adversarial corpus; every planner (`plan_ship`, `plan_done`, …)
asserting exact `Op`s against a hand-built `Snapshot` and a `Facts` literal — this is what the pure
planner buys, and it is the single best testing seam in the design; rung *parsers* against
recorded git stdout strings.

**The real half.** `tests/common/mod.rs::TestRepo` builds a real temp git repo with a real bare
`origin`, real commits, real worktrees, and the six real merge shapes.

```rust
// tests/common/mod.rs — FROZEN, foundation-owned
pub struct TestRepo { pub root: PathBuf, pub origin: PathBuf, _tmp: tempfile::TempDir }
pub struct Run { pub code: i32, pub stdout: String, pub stderr: String }

impl TestRepo {
    /// Builds the repo ONCE into a process-wide template dir, then `fs::copy`s it
    /// per test (~4ms vs ~30ms for `git init` per test). With 9 agents each running
    /// the suite on every save, this is the difference between a fast inner loop
    /// and a suite nobody runs.
    pub fn new() -> TestRepo;
    pub fn with_merges() -> TestRepo;           // + common::merges::all()
    pub fn worktree(&self, name: &str) -> PathBuf;

    /// In-process for speed; `cli_smoke.rs` shells the real binary for the argv /
    /// exit-code / `ks`-alias wiring the in-process path skips.
    pub fn ks<I, S>(&self, args: I) -> Run where I: IntoIterator<Item = S>, S: AsRef<str>;
    pub fn ks_in<I, S>(&self, cwd: &Path, args: I) -> Run;
    pub fn json<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> T;

    pub fn write(&self, rel: &str, body: &str);
    pub fn read(&self, rel: &str) -> String;
    pub fn commit(&self, msg: &str) -> String;
    pub fn push(&self, branch: &str);
    pub fn gh_fixture(&self, name: &str, json: &str);   // -> $KANSPEC_GH_FIXTURES
}

// tests/common/merges.rs — the six shapes, each a REAL merge into a REAL origin
pub enum Shape { TrueMerge, SquashGitNative, SquashGhTitleOnly, Rebase, SquashOneCommit, Never }
pub fn all(repo: &TestRepo) -> Vec<(Shape, TicketId, ExpectedStatus)>;
```

**Determinism comes from exactly three env overrides, read in `Ctx::open`:** `KANSPEC_NOW`,
`KANSPEC_ACTOR` (+ `KANSPEC_ACTOR_KIND`), `KANSPEC_ID_SEED`. **The one and only mock seam in the
crate is `KANSPEC_GH_FIXTURES`** — you cannot create a real GitHub PR in a test, and rung 2 is the
only rung that catches the title-only squash. **Git itself is never mocked**, and there is no
`trait GitBackend`, because the second implementation does not exist. `insta` filters normalizing
SHAs, timestamps and generated ids are written with the **first** snapshot, not retrofitted.

One more env var exists and is deliberately NOT in that list, because it is read by the installed
shell hooks rather than by `Ctx::open` and it changes no answer kanspec computes: **`KANSPEC_BIN`**
overrides the binary path a hook invokes (hooks default to the name the user ran `init` as, and
no-op silently when it is not on `PATH`). It exists so `tests/setup_hooks.rs` can point a real git
hook at `target/debug/kanspec`; nothing in the product reads it. Owner: S7, documented in
`docs/config.md`.

**How each slice tests in isolation.** Every slice's public surface exists as a signature after
wave 0, so a slice compiles and unit-tests against `unimplemented!()` neighbours from hour one.
Only *runtime* integration waits on a dependency, and the gate for each slice (§10) names exactly
which test proves it.

**Invariant tests own their own file so they cannot be quietly weakened:**

| File | Proves |
|---|---|
| `single_write_path.rs` | source grep: no `fs::write\|fs::rename\|File::create\|OpenOptions\|fs::remove\|fs::create_dir` outside `store.rs` + the 3-file allowlist |
| `purity.rs` | source grep: `derive.rs` imports no `std::fs`, no `std::process`, calls no `Utc::now()` |
| `proof_is_sealed.rs` | source grep: no `impl (Deserialize\|Default\|From<.*>) for MergedProof` — it *will* be tempting the first time someone wants a fast `status` |
| `cache_wipe.rs` | `rm -rf .kanspec/cache` changes no rendered state except freshness stamps. The correct **behavioural** proof of invariant 1 — it holds regardless of how a derived fact got there, which no type and no grep can do |
| `invariants_rules.rs` | `prime` stdout `starts_with` `rules` stdout, on the real binary, across N scopes |
| `transition_table.rs` | exhaustive (State × Verb) agreement; `replay` matrix incl. the repair reset |
| `doctor_replay.rs` | a hand-edited `state:` fails; a legal trail passes; `repair` recovers |
| `worktree.rs` | every mutating verb run from a linked worktree lands in the primary `.kanspec/` |
| `scan_ladder.rs` | all six merge shapes → exact `MergeStatus` **and** `Method`, incl. the two that must be `unknown` |
| `lock.rs` | 20-process contention; `kill -9` mid-transaction releases |
| `fm_bytes.rs` | byte-stability, 100-edit idempotence, CRLF, adversarial corpus |
| `json_matrix.rs` | `every_command_supports_json`, walked mechanically off the finished clap tree |
| `setup_hooks.rs` | foreign hook preserved through install/uninstall; `core.hooksPath` respected; `.d/` dispatch order |
| `cli_well_formed.rs` | `Cli::command().debug_assert()` — **not optional**: `global + required` is a debug-only assert compiled out of release |

---

## 10. File ownership map

**Wave 0 (Foundation, ONE agent, serialized, ~3h) delivers a COMPILING SKELETON**, not a document:
every file in §1, every `pub` signature, every doc comment, every `impl Render for X` stub, bodies
`unimplemented!("S4")`. Plus `Cargo.toml`, `build.rs`, the complete clap tree from DESIGN.md's CLI
reference, the complete `dispatch` match wiring every verb (v0.1 **and** v0.2) to a real `cmd::*`
signature, and `tests/common/`.

**Exit gate:** `cargo check --all-targets` clean · `cargo clippy --all-targets` clean ·
`cli_well_formed.rs` passes · `kanspec --help` and `ks --help` print the real CLI reference ·
every verb exits 1 with `not implemented (owner: S4)` · `TestRepo` actually builds a temp repo with
a bare origin and the six merge shapes.

| # | Slice | Owns (exclusive write) | Depends on (reads only) | Must NOT touch |
|---|---|---|---|---|
| **F** | **Foundation** | `Cargo.toml`, `build.rs`, `rust-toolchain.toml`, `src/lib.rs`, `src/bin/*`, `cli.rs`, `ctx.rs`, `error.rs`, `out.rs`, `paths.rs`, `config.rs`, `ids.rs`, `keys.rs`, `logentry.rs`, `model.rs`, `transitions.rs`, `plan.rs`, `cmd/mod.rs`, `tests/common/**`, `tests/fixtures/**` *except* `tests/fixtures/gh/**` (S2's, see below), `tests/cli_well_formed.rs`, `tests/cli_smoke.rs` | — | any slice file after wave 0 |
| **S1** | **Write path** | `fm.rs`, `lock.rs`, `store.rs`, `tests/{fm_bytes,lock,single_write_path}.rs` | F | everything else |
| **S2** | **Git** | `git.rs`, `gh.rs`, `tests/worktree.rs`, `tests/fixtures/gh/**` | F | everything else |
| **S3** | **Scan** | `scan.rs`, `cache.rs`, `cmd/scan.rs`, `cmd/repair.rs`, `tests/{scan_ladder,proof_is_sealed}.rs` | F, S1(store), S2(git,gh) | everything else |
| **S4** | **Derive + doctor** | `derive.rs`, `doctor.rs`, `cmd/doctor.rs`, `cmd/status.rs`, `tests/{purity,transition_table,doctor_replay,cache_wipe}.rs` | F, S1, S3(cache types) | everything else |
| **S5** | **Ticket verbs** | `triage.rs`, `cmd/ticket.rs`, `cmd/flow.rs`, `cmd/done.rs`, `tests/lifecycle.rs` | F, S1, S2, S3(`Landed`,`proof_for_done`), S4(derive) | everything else |
| **S6** | **Knowledge + rules** | `rulesdoc.rs`, `project.rs`, `cmd/{spec,quirk,decision,rules,prime,features}.rs`, `tests/invariants_rules.rs` | F, S1, S4 | everything else |
| **S7** | **Setup / hooks / docs** | `hooks.rs`, `setup.rs`, `instructions.rs`, `ci.rs`, `cmd/{init,setup}.rs`, `docs/**`, `tests/setup_hooks.rs` | F, S1, S2 | everything else |
| **S8** | **Board + server** | `board.rs`, `server.rs`, `cmd/{board,up}.rs`, `assets/**`, `tests/{board,json_matrix}.rs` | F, S1, S4, and every `cmd::*` fn | everything else |
| **V2** | **Proposals + review** (v0.2) | `cmd/{proposal,comment,landcheck}.rs` | F, S1, S3, S6 | everything else |

**No file appears twice.** Every file in §1 has exactly one owner. There is **no split ownership
within a file** — the wave-0 commit *hands each file over* whole, signatures included; whoever
fills the body owns the signature too. ✅ (Sealed Keel's `// ── keel ──` marker comment put two
owners in one file, the exact hazard its plan claimed to eliminate.)

**The five rules that make this actually disjoint** — each removes a *named* merge magnet:

1. **No agent adds a `mod` line.** `lib.rs` and `cmd/mod.rs` declare every module in wave 0,
   including every v0.2 module.
2. **No agent adds an error variant.** The 8 shapes are closed; new refusals are
   `KsError::gate(code, msg, fixes![..])` in the agent's own file.
3. **No agent adds an output variant.** Each command's payload struct **and** its `impl Render`
   live in that command's own file; `out.rs` never grows.
4. **No agent adds a CLI arg.** The full clap tree ships in wave 0 straight off DESIGN.md's CLI
   reference table (already declarative and complete; the recon proved every shape parses,
   including the `--spawn`/`--no-followups` group and `allow_hyphen_values` on every free-text
   reason). v0.2 subcommands ship `#[command(hide = true)]`.
5. **No agent edits `Cargo.toml`.** Every crate the v0.1 *and* v0.2 cut needs is pinned in §8.

A missing type or flag is a **request to F**, batched between waves. Budget one contract-change
round per wave; fewer than five should be needed across the build.

### Integration order — 6 rounds

| Round | Lands | Gate ("done" looks like) |
|---|---|---|
| **0** | **F alone.** Nobody else has started. | `cargo check --all-targets` + clippy green; `cli_well_formed` passes; `TestRepo` builds the six merge shapes |
| **1** | **S1 + S2 in parallel** (no shared file, no shared type they both define) | `fm_bytes.rs` (byte-identity + 100-edit idempotence on the adversarial fixture) · `single_write_path.rs` · `lock.rs` (20-process contention, `kill -9`) · `worktree.rs` (every verb from a linked worktree hits primary) |
| **2** | **S3 (ancestry rung + `cache.rs` first, then rungs 2–4)** and **S4 in parallel** | `scan_ladder.rs` all six shapes with exact `Method`, **two landing on `unknown`** · `proof_is_sealed.rs` · `transition_table.rs` (all 40 pairs) · `doctor_replay.rs` · `purity.rs` |
| | *Ancestry moves this early on purpose: the `done` gate's primary path must be real from birth, or three milestones of dogfooding tune the UX against `scan --confirm` and wear the human override smooth.* | |
| **3** | **S5** — the walking skeleton | `lifecycle.rs`: `new → ready → start --worktree → ship --pr → (REAL squash merge) → scan → done`, run **from inside a linked worktree**, asserting every write landed in the primary `.kanspec/` and the `## Log` replays clean. **Dogfooding starts here.** |
| **4** | **S6 + S7 in parallel** | `invariants_rules.rs` byte-identity across N scopes on the real binary · `cache_wipe.rs` · `setup_hooks.rs` (`setup claude --remove` restores a pre-existing husky-style hook exactly; `core.hooksPath` honoured) · **D-20 MUST BE WIRED HERE**: `cmd/scan.rs` carries a marked comment at the exact call site where `project::plan_regenerate` (S6's, `todo!()` through round B, so calling it would panic every `scan`) pushes its two `Op`s into the same transaction. Close it or the committed `KANSPEC-*.md` projections silently rot — the exact failure the projections exist to prevent — and add the test the corrections doc asks for: *a scan after a spec edit rewrites the root files*. |
| **5** | **S8** — last, because it consumes every view and every `cmd::*` fn and adds no new semantics | `json_matrix.rs` · `board.rs` insta snapshots · manual: `up` with two SSE tabs open, **Ctrl-C exits in under a second**; a CLI `start` in another terminal refreshes the board; a POST and the equivalent CLI verb produce byte-identical files |
| **6** | Release cut | `cargo-dist`; `init --refresh-hooks` against the dogfood repo; **one manual run against a real GitHub squash-merged PR** before the ladder is trusted |

**Rebase discipline:** each slice rebases on the integration branch at the start of every round.
Because ownership is disjoint and `Cargo.toml` is complete up front, rebases are conflict-free by
construction.

**If fewer agents are available**, the honest collapse — chosen so adjacent slices merge without
changing any file's owner — is S1+S2 (round 1), S3+S4 (round 2), S6+S7 (round 4), giving F + 5.

### Non-negotiables every slice must honour

- Handlers are `fn(ctx: &Ctx, a: &XArgs) -> Result<XReport>`; `XReport: Render`; handlers **never**
  print and never call `process::exit`.
- Mutations go through `Store::transact` and a pure planner. **Every subprocess, network call and
  clock read happens before `transact`**, packaged into a `Facts` value.
- Every `KsError` you construct names its fix. `Fixes` makes this impossible to skip.
- Every new frontmatter key is a **request to F** for a `keys.rs` variant, never a `&str`.
- axum 0.8 routes use `{id}`, not `:id` — `:id` **panics** at `Router::route()`.

---

## 11. Decisions resolved

### Judge disagreements (one line each)

| # | Split | Resolution |
|---|---|---|
| J-1 | Winner: 2 judges Skeleton-First, 1 One Gate | **Skeleton-First**, because it is the only entry whose headline safety type compiles (E0742/E0277 reproduced against the others) and the only one where adding a command grows no shared file. |
| J-2 | Typestate (`At<'t,P>`/`Change<'t,P>`) vs const table | **Const table.** `P` is never dispatched on, `Vec<Change<'t,?>>` is unrepresentable exactly when v0.2's `close` needs to batch N transitions, and Sealed Keel concedes the *seal*, not the typestate, earns its keep. |
| J-3 | `Report` mega-enum vs per-command `impl Render` | **Per-command**, in the command's own file — the churniest surface in the crate must not be single-owner. |
| J-4 | Planner purity | **Pure with `Facts`.** The idea is grafted; `&Ctx` in a planner is not — it would repeat One Gate's own contradiction of shelling out to git inside the lock. |
| J-5 | Lock: flock vs O_EXCL pidfile | **flock**, with One Gate's `LockOwner` JSON body written *after* acquiring for the diagnosable message. |
| J-6 | Error granularity | **Closed 8 shapes** (Skeleton-First, so `error.rs` never grows) **with** structured `GateDetail` payloads for the two errors whose output is the product (Sealed Keel). |
| J-7 | Exit codes: 0/1/2/64 vs full sysexits | **0/1/2/64/69/70.** "Agents branch on zero/nonzero/2" is right for agents, but a wrapper script must distinguish "no `.kanspec/` here" from "gate refused". |
| J-8 | Cache vs proof | **Split.** `MergeFact` is a plain `Deserialize` badge DTO; `MergedProof` is sealed and minted fresh at the gate. This is the exact boundary Sealed Keel got wrong. |

### DESIGN.md ambiguities resolved

| # | Ambiguity | Decision |
|---|---|---|
| D-1 | Build plan says "illegal transitions unrepresentable at compile time" | **Rejected as written.** Ticket state arrives from a hand-editable file at runtime; typestate would require a fallible downcast at every boundary. `require(from, verb)?` gives the same typed refusal. The compile-time budget is spent on `MergedProof`, `TicketKey`, `HumanActor`, `KanspecDir`. |
| D-2 | Crate list names `gray_matter` | **Removed.** It cannot serialize and mutates content. `src/fm.rs` (first-party) + `serde_yaml_ng` read-only. |
| D-3 | Ladder rung 4 "patch-id — last resort for squashes" | **Backwards.** Relabelled *rebase/cherry-pick detection*; a `+` line is `Unknown`, never `NotMerged`. |
| D-4 | Ladder order not fully specified | **Two guards added** before rung 1: object-exists, and `rev-list --count main..head != 0` (a fresh `start` branch is trivially an ancestor — a verified false MERGED). |
| D-5 | "`prepare-commit-msg` (per-branch, set by `start`)" | **Git has no per-branch hooks.** One repo-wide hook dispatching on `branch.<name>.kanspec-ticket` (read through `hooks::BRANCH_TICKET_KEY` / `hooks::branch_ticket_key`, which `start` must use for the same spelling), skipping `$2 ∈ {merge, squash, commit}`. Installed by `init`, not by `start`. **Round-A correction: it is a PAIR of hooks, not one.** `prepare-commit-msg` runs *before* the editor, so on an interactive commit the message is still empty — and stamping it makes it non-empty, silently destroying git's "an empty message aborts the commit" (reproduced against real git: `GIT_EDITOR=true git commit` committed with the message `Kanspec: t-9c41`). So `prepare-commit-msg` stamps only a message that already has content (`-m`/`-F`/`-t`), and a companion **`commit-msg`** hook, which runs *after* the editor, stamps the rest. Neither stamps twice; both skip a merge, a squash, and anything below a `git commit -v` scissors line. `hooks::HOOKS` therefore has four entries: `post-merge`, `post-checkout`, `prepare-commit-msg`, `commit-msg`. |
| D-6 | "resolves `git rev-parse --git-common-dir`" | Insufficient: it is *relative* in the primary worktree. `--path-format=absolute` + `git_dir == common_dir` **first** + `worktree list` fallback + a sanity check that **refuses** rather than guessing. |
| D-7 | Hooks installed (implied `.git/hooks`) | **Resolve via `rev-parse --git-path hooks`**; `core.hooksPath` (husky/lefthook) makes `.git/hooks` inert. Install kanspec as the entrypoint, move any pre-existing hook to `<hook>.d/10-<name>`. Naive append is unsafe two ways (`exit 0` starvation; missing trailing newline). |
| D-8 | `head:` "survives branch deletion" | **Narrower than claimed:** ~2 weeks post-reflog-expiry, then gc removes it, and it only rescues rung 1 — which only fires when the SHA is reachable from main anyway. Still recorded: it is `cherry`'s input, the CI-by-SHA key, and `--explain` provenance. |
| D-9 | Spec `code:` globs feed the tripwire | Pathspecs are **cwd-relative** → `Pathspec` newtype always emits `:(glob,top)`. Merge counting uses `--first-parent` (2 commits vs 1 merge in the recon repo), or the tripwire over-fires by the size of each PR. |
| D-10 | Staleness "counter resets" on confirm | **Never an accumulated counter** — recomputed at read time; a counter in a disposable cache silently resets on wipe and *under*-fires. The human's "no behavior change" is an asserted act → `stale_ack: {sha, at, by, why}` in the **spec frontmatter** (git-tracked, survives `rm -rf cache/`). |
| D-11 | `scan --confirm` storage unspecified | **The ticket's `## Log`**, as an attributed `Verb::Confirm` line — not a cache entry. A human attestation must survive a cache wipe and be visibly signed. |
| D-12 | No verb exists for repairing a broken log | **Added `Verb::Repair`** (`kanspec repair <id> --why`), the one verb whose logged state is authoritative in `replay`. Without it, replay-on-commit turns any imported or already-broken repo into a permanently unwritable one. |
| D-13 | Open question 1 — auto-commit board state | **`sync = "batch"` default**; `status` reminds when N tracker changes are pending. `sync = "commit"` is one config line. |
| D-14 | Open question 3 — landcheck strictness | **Opt-in** (`[hooks] landcheck = false`), installed but config-gated, v0.2. |
| D-15 | Open questions 2 & 4 | **Ancestry-first** (recon: exact, cheap, and gh is rung 2 — the only rung catching a title-only squash). **No PR importer** in v0.1 or v0.2. |
| D-16 | Bare `kanspec` / bare `kanspec comment` exit code | **64**, not 0 — an agent that runs an incomplete command must not think it succeeded. |
| D-17 | `ready` when a dep is in-main but not `done` | **Satisfied** by terminal *or* in-main. **Dropped counts as satisfied** (blocking forever is worse); `doctor::check_orphan_deps` warns on a dep pointing at a dropped ticket. |
| D-18 | Invariant 8 ("agents never self-accept") had no mechanism | **`HumanActor`**: private field, constructor refuses `Actor::Agent`. `plan_accept`/`plan_revoke` take `&HumanActor`, so an agent session cannot call them. |
| D-19 | `comments.jsonl` "id-dedupe on read" — dedupe key unspecified | **`(id, op, at)`**: one `cm-` id legitimately carries `comment` + `reply` + `resolve` rows. |
| D-20 | `KANSPEC-*.md` regeneration timing | On `scan`, and on any `transact` that touched a spec or a decision (`Op::WriteGenerated`). **Amended in round C (see D-34): its OWN short transaction immediately after, not inside the caller's lock** — a planner sees the pre-plan snapshot, so regenerating inside would publish a projection permanently one write behind. |
| D-21 | Branch / worktree naming | `ks/<id>-<slug>` (`branch_prefix` configurable), worktree `<worktree_dir>/<id>`. `worktree add --no-track` — without it a later `git push` from the ticket branch targets **main**. |
| D-22 | Server snapshot freshness | `up` holds `Arc<Ctx>` and a `rev`-stamped memoized snapshot that `transact` **publishes synchronously** on write; the watcher only invalidates. A POST's own refetch can never see the pre-write snapshot. **AMENDED IN ROUND D — see D-40. The memo is NOT built, and the `Arc<Ctx>` is an anchor rather than a reused context: a `Ctx` held across requests writes its own construction time into every ticket Log it touches, which is a correctness bug, not a slower path.** |
| D-23 | SSE payload granularity | `{rev, n}` **only**; the SPA refetches `/api/board`. macOS FSEvents coalesces create+remove+modify for one delete, so any event-kind-derived delta is a bug farm. |
| D-24 | `rusqlite` in v0.1 | Behind a **non-default** `ci-homerunner` feature until `ci.rs` lands. Bundled SQLite is the largest clean-build cost in the set for code v0.1 never calls. |

### Round-B resolutions (integration of S3 + S4)

| # | Question | Decision |
|---|---|---|
| D-25 | `Verdict::NotLanded` is effectively **unreachable** — rung 4's only non-merged outputs are `+` (`SquashSuspectedNoGh`) and an empty `cherry` (`ConflictingSignals`), and `git cherry` never yields `Tri::No`. So `ScanReport.not_landed` is always empty and every in-flight ticket badges `unknown`. | **Kept, deliberately.** This is R-4 arriving as predicted, and it is the honest answer: a `+` line cannot distinguish an unmerged branch from a multi-commit squash (D-3). The plausible narrowing — a `+` becomes a real `NotLanded` when `gh` DID answer and reported no merged PR for the branch — is **v0.2 at the earliest**, because it is wrong for a branch merged by hand with no PR, which is exactly the confident-wrong-answer invariant 2 forbids. `tests/common/merges.rs::Shape::expected()` was corrected in round B (it predicted `NotMerged` for `Never`); **two** of the six shapes land on `unknown`, matching §10's round-2 gate. |
| D-26 | `doctor --fix` cannot strip a hand-edited derived key (`merged: true`), because removing it needs an op that NAMES the key and `keys::Key` deliberately has no such variant. Proposed: `Op::RemoveFields { entity, keys: Vec<String> }` over RAW key names. | **Rejected for v0.1 and v0.2.** That op is a hole straight through invariant 1's type-level defence — `Key` has no derived variant *precisely* so that no code path can name one — and it would be added to make a lint auto-fixable. The finding stays `fixable: false` and names the file and the line to delete, which is what invariant 9 actually asks for. R-2 already says the seals bind the tool and the Log binds the human; this is that boundary, working. |
| D-27 | `doctor::check_frontmatter_writable` cannot run the real `fm::writable()` check: entities carry `body` (everything after the closing fence) but not the raw frontmatter TEXT. Proposed: add `fm_text: String` to all five entity structs. | **Deferred.** It touches `model.rs` (F) *and* `store.rs` (S1) *and* every entity literal in every slice's unit tests, mid-build, for a check the write path already enforces — `fm::set` hard-errors on a multi-line value before any byte moves. S4 implemented the pure subset visible from `extra` (unindexable keys, multi-line values) and documents exactly what it does and does not catch. Revisit with the v0.2 `doctor` cut. |
| D-28 | `doctor::check_dead_globs` covers spec `code:` globs only, while its registry `about` promised quirk `paths:` and decision `scope:` too. | **`about` corrected to match reality** in round B. `cache::SpecAnchor.dead_globs` is the only glob liveness `scan` records; covering the other two needs a glob-liveness map in `GitState` (S3's `cache.rs`) — v0.2. A registry that promises more than it checks is worse than one that checks less. |
| D-29 | `doctor::check_immutable_decisions` needs a git diff of an accepted decision's body against its last commit, which `RunCheck = fn(&Snapshot)` cannot do. | **Deferred, half implemented.** The record half (accepted **and** `superseded_by` set) is checked purely; the body-diff half needs `scan` to record a per-decision body hash. Do not move the check off the pure registry — `purity.rs` holds `doctor.rs` to `fn(&Snapshot)` on purpose, and a check that shells out is a second, slower, un-unit-testable copy of `scan`. |
| D-30 | The in-main and settling dwells anchor on ticket **activity** (last log entry / last branch commit), not on when the work actually landed. | **Correct as built.** The cache records `checked_at` — when the ladder *ran* — not a merge date, and anchoring on `checked_at` would let `up`'s 60s scan loop reset the tripwire forever, making it unfireable. `derive.rs` has a test named for exactly that. If `scan` ever records a real merge timestamp, switch the in-main dwell to it. |

### Round-C resolutions (integration of S5 + S6 — the walking skeleton + knowledge)

| # | Question | Decision |
|---|---|---|
| D-31 | `Store::transact(verb: Verb)` — S3, S5 and S6 each filed the same request for `Option<Verb>`, because a plan that transitions nothing had to pass `Verb::Confirm` as filler. | **Granted.** Not cosmetic, as round B assumed: **10 of 20** call sites were filler, so under `sync = "commit"` a `spec new auth` committed as `kanspec: confirm auth`. `Confirm` is the human merge override (D-11); borrowing it made git history assert something false. `Some(v)` iff the transaction IS ticket verb `v` (`Op::Transition`, plus `new`'s genesis); `None` otherwise, committing as `kanspec: update <id>`. §2.13 updated. |
| D-32 | `Yv` cannot express `spec.stale_ack`, whose type `model::StaleAck { sha, at, by, why }` was already frozen in §2. S6 added `Yv::Map`. | **Granted, verified empirically.** No other spelling works: a flow SEQUENCE is rejected by serde, and `Yv::Str("{…}")` is single-quoted by `emit` (`{` is in `plain_ok`'s deny list) and reads back as a String — which makes the entire spec unloadable and takes `rules`, `prime` and the feature map with it. **Flow style, never a block map**, so `fm::index` reports `multiline: false` and a second `features --confirm` is a `Replaced` rather than R-9's refusal. §2.14 updated. |
| D-33 | `Op::writes_tracked_file` returned true for `WriteGenerated`, which D-20's wiring surfaced. | **Granted, and load-bearing** — verified by reverting it, which fails `scan_ladder.rs::a_scan_commits_nothing_under_sync_commit_…` on the nose. `commit_kanspec` is scoped to `:(glob,top).kanspec/**`, so counting a repo-root projection never commits it; it only makes `scan` sweep a human's pending tracker edit into a commit labelled after the scan. §2.10 updated. |
| D-34 | D-20 says regeneration rides "inside the same lock" as the write that changed a spec or decision. `project::regenerate` runs as its own short transaction immediately after. | **Deviation ACCEPTED; D-20 amended.** A planner sees the snapshot as it was BEFORE its own plan, so regenerating inside the closure would publish a feature map permanently one write behind — a brand-new spec missing until some later verb ran, and a post-`scan` `Fresh?` column computed from the pre-scan `GitState`. That is the exact rot the projections exist to prevent. Cost is R-1's window, one lock cycle wide, self-healing on the next verb. |
| D-35 | S5 invented a non-DESIGN `ship` gate: refuse when the branch is 0 commits ahead of main. | **Kept.** Without it `start` → `ship` records main's own SHA as `head:`, and rung 1 then answers MERGED — a *verified false positive for work that never happened*, the single worst answer this tool can give. Guard 0b covers only the branch-tip case and cannot see this one by construction. It fires **only on a measured `Tri::Yes(0)`**; `Unknown` never refuses. It lives in the handler because the planner is pure and `ShipFacts` carries no commit count — safe today because `cmd::flow::ship` is the only caller, including from the server (§2.16). |
| D-36 | S5 made `start` check the primary worktree out onto the ticket branch — not in DESIGN, which says only "creates branch (+ worktree)". | **Kept, and REPAIRED at integration.** Keeping it: without a checkout an agent following the CLAUDE.md snippet commits to main and the ticket-branch model breaks silently, which the dogfood run reproduced. Repairing it: as shipped the guard ran `git status --porcelain -uno` *after* `transact`, so in any repo that COMMITS `.kanspec/` (as DESIGN does) the ticket file the verb had just rewritten was the only dirty entry and the switch **never fired on any repo**. `.kanspec/**` is now excluded from the check and the check is hoisted above the write. Pinned both directions by `lifecycle.rs::start_moves_the_primary_onto_the_branch_only_when_the_source_tree_is_clean`, which commits AND pushes the tracker first — with an untracked `.kanspec/` the bug is invisible, which is how it shipped green. |
| D-37 | `rules --adopt` needs an op that rewrites one line inside an entity BODY (`Op::ReplaceLine`). | **Rejected for v0.1; stays a typed refusal.** DESIGN's build plan puts `rules --audit/--adopt` in v0.2, and nothing else in v0.1 needs the op. S6 ships `--adopt` as an exit-1 gate naming three real fixes rather than a no-op exit 0 — an exit 0 that changed nothing is how a human comes to believe the audit is clean. `rulesdoc::ADOPTED_TOKEN` and the audit's `adoptable` flag are in place, so v0.2 needs only the op. |
| D-38 | `cache::MergeFact.why` stores `git::Unknown::badge()` — the COMPLETE `unknown (…)` text — while `derive::Badge::text` supplies its own wrapper. | **Fixed at the seam (integration).** Round C's `ls` was the first surface to render a badge for a ticket with no branch, and it printed `unknown (unknown (no branch or head SHA recorded) · checked 7s ago)`. `derive::bare_reason` unwraps once, so exactly one layer owns the wrapper; it is total and idempotent, so it stays correct if the cache is ever changed to store the bare half. `cache.rs`'s doc now states the field is complete badge text. |

### Round-D resolutions (integration of S8 — the board and the server)

| # | Question | Decision |
|---|---|---|
| D-39 | `Op::WriteGenerated` was documented `KANSPEC-*.md ONLY`, but `board --export board.md` now uses it — it is the only typed op that writes arbitrary bytes to an arbitrary path, and the wave-0 stub for `cmd/board.rs` explicitly routed `--export` through `Store::transact`. | **Granted; the comment was under-describing the op, and the comment is what changed.** Nothing behaves wrongly: `Plan::validate` already permits it, and `writes_tracked_file()` correctly excludes it (a repo-root file is outside `commit_kanspec`'s `:(glob,top).kanspec/**` pathspec, D-33). The op is safe for exports for the same structural reason it is safe for projections — it cannot name a `Key`, so no seal is bypassed by using it. §2.10 and `plan.rs` now say "generated bytes at a path the tracker does not own", and say plainly that it is not a general file writer: an *entity* is written with `CreateEntity`/`SetFields`, which are the ops the seals apply to. |
| D-40 | D-22 gives `up` one `Arc<Ctx>` plus a `rev`-stamped memoized snapshot. S8 built the `Arc<Ctx>` as an anchor only, constructs a fresh `Ctx` per request, and did not build the memo. | **Deviation ACCEPTED; D-22 amended. This is a correctness fix, not a dropped optimisation, and the reasoning was verified against the code.** `Ctx.now` is stamped **once**, by `detect_now()` in `Ctx::open` (`ctx.rs`), and `load_snapshot` copies it into `Snapshot::now` (`store.rs`) while `transact` stamps it into `LockOwner` and every `## Log` line. A server holding one `Ctx` for a working day would therefore (a) compute every dwell, every STALLED window and every `checked Nm ago` against its own start time, and (b) **write that start time into the Log of every ticket a POST moved** — so the moment a CLI verb in another terminal had logged a later time, `replay`'s monotonicity check would make that ticket permanently unwritable until `kanspec repair`. Cost of the fix is two `git rev-parse`s and a config read per request (~10ms on loopback). Building the memo later requires a `Ctx` whose clock is *not* frozen at construction, which is an **F** change to `ctx.rs`; nobody should attempt it before profiling says the reload hurts (R-6). |
| D-41 | Every hidden v0.2 arm (`propose`/`review`/`approve`/`close`/`abandon`, `comments`/`comment`/`promote`/`expire`, `landcheck`) was `todo!()`, so running one exited **101** with a panic backtrace, no `--json` envelope, and no fix. | **Closed at integration.** 101 is outside §2.1's `code` set entirely, and to a wrapper script it is indistinguishable from a crashed tool — the opposite of invariant 9. Each handler now returns `KsError::gate(…)` naming what is missing and what to run instead, following `ci::not_yet_v02`'s round-A precedent (exit 1, full envelope, fix list). `landcheck` gets this treatment most urgently: it is the only route to exit **2**, and an unwritten Stop hook that blocked every agent session from ending would be far worse than one that says it is unwritten. The `Render` impls for those reports lost their `todo!()`s too — they are unreachable while the handlers refuse, but a latent panic in dead code is still a latent panic. Pinned by `json_matrix.rs::every_v02_arm_refuses_in_the_documented_exit_range` and `::the_unimplemented_landcheck_never_blocks_a_session`. **This is the shape any future unimplemented verb must take**; a `todo!()` in a dispatchable handler is now a test failure. |

### Round-E resolutions (integration of the five adversarial-audit fixes)

| # | Question | Decision |
|---|---|---|
| D-42 | `src/out.rs` is "frozen after wave 0", but the `--color always` fix promoted `visible_len` to `pub` and added `pub fn pad_visible`. Blessed? | **Granted, and §2.12 now records both.** The fix's whole point is that `pad_visible` becomes the ONE padding primitive — a private helper cannot serve a call site outside `out.rs`, and `tests/render_color.rs` is an integration test that can only reach it through the public API. Widening a frozen file by two pure, total functions that add no state and no variant is the cheapest possible way to close a bug *class* rather than an instance. Note the guarantee is narrower than the doc comment implies: `pad_visible`'s "any future column in this file" is scoped to `out.rs`, while `paint` is `pub` and called from ~35 sites across 15 files, so a future `format!("{:<10}", paint(…))` **elsewhere** would reproduce the bug uncaught. No such site exists today (audited: the only files where a painted string and a width specifier coexist are `error.rs` and `cmd/init.rs`, and both feed the specifier unpainted). A crate-wide guard is v0.2. |
| D-43 | `impl Verb` gains `command`/`command_as`, and `Verb::command()` reads the process-wide `crate::cli::invoked_as()` rather than `ctx.invoked_as`. | **Granted; §2.4 now records both.** `require`'s signature is frozen and carries no `&Ctx`, and it is called from the write path, the board's drag-to-verb and `doctor`; threading a `&str` through all of them to spell one fix line is a contract change out of proportion to the fix. The two values are identical by construction (`ctx.rs` sets `invoked_as: cli.invoked_as()`), and `instructions.rs` already reads the `OnceLock` inside a `fix!`. `command_as` is the seam tests pin both spellings through without racing set-once state. |
| D-44 | Should `board.rs` and `derive.rs`'s hand-built `format!("kanspec start {id}")` be routed through `Verb::Start.command(id)` too? | **Rejected at integration, as scope.** They are correct today (`start` needs no flag) and wrong only in hardcoding `kanspec` for a `ks` user — which is one instance of **224** hardcoded `"kanspec "` strings in `src/`. Fixing 2 of 224 buys nothing and makes the remaining 222 look deliberate. This is a single mechanical sweep (route every user-facing command string through `invoked_as()`), and it wants its own change with its own test, not a rider on a transitions fix. **Round F update — the REFUSAL half is now done, mechanically.** `Fix::cmd` routes through `out::spoken`, which rewrites the command word to `invoked_as()` at CONSTRUCTION, covering all ~123 `fix!("kanspec …")` sites without editing one of them. Construction, not render: `Fix` is `#[serde(transparent)]`, so rewriting per surface would let the human line and the `--json` fix list name different commands — the one drift `Render: Serialize` exists to forbid. The rewrite fires ONLY in command position (string start, or immediately after a backtick) and only where the word ends there, because a naive `replace` corrupts real advice: it turns `.kanspec/tickets/t-31aa.md` into a path that does not exist and rewrites the message in `git commit -m "kanspec: sync"`. Both corruptions are pinned as tests. STILL OPEN: ~94 hardcoded `"kanspec …"` strings that are NOT `Fix` — report `next: Vec<String>` and `Finding.fix: String` fields in `derive.rs` (16), `doctor.rs` (20), `board.rs` (7), `cmd/repair.rs` (5) and ~10 more files. Those must be fixed where the String is BUILT, never at the render layer, for the same human/JSON reason. Consequence to know: a `ks` user now sees `ks` on Fix-bearing surfaces and `kanspec` on the String ones — mixed rather than uniformly wrong. Strictly better (both binaries ship, so every line still runs) but visibly inconsistent until the sweep lands. |
| D-45 | `repair` into a terminal state now demands log-borne evidence, so an imported repo full of `done` tickets with no `## Log` and no SHAs cannot be attested into `done` at all. | **Accepted as correct, and it is the one behaviour change most likely to be argued with.** `done` is the single state this product computes from git rather than accepting on anyone's word; an escape hatch strictly more permissive than the gate it bypasses is not an escape hatch, it is the gate's absence. The importer's route is to emit the close as a `done` line in the imported `## Log`, which the guard accepts by design. Note the residual: this is **detection, not prevention** — a human with an editor can still hand-write a `repair` line, and `doctor::check_attested_state`'s Error grade is what catches that. Same bargain as R-2. |
| D-46 | The attested-close badge reuses `Badge::Unknown { why: "attested done by …" }` instead of a new `Badge::Attested`. | **Correct as built; do not add the variant in v0.1.** A new variant changes the `--json` shape of `ls`/`show`/`board`, which agents branch on. `Unknown` is not a euphemism here, it is the literal truth: git proved nothing, a person said so, and the `why` says which person and when. If v0.2 wants a first-class variant it is a one-arm change to `Badge::text` plus the existing precedence block. |
| D-47 | The committed `KANSPEC-FEATURES.md` lost its `Fresh?` column. Product decision, taken by a fix agent. | **Upheld.** The column was computed from `.kanspec/cache/gitstate.json`, which `init` gitignores — so a git-tracked file was a function of a per-machine disposable artifact and two clones of one commit regenerated different bytes. It could not be made deterministic while remaining freshness, because freshness *is* cache-derived; the only git-tracked freshness signal is `stale_ack`. DESIGN.md's mock-up is corrected rather than the code. Cost: a one-time column removal in the next diff for anyone who committed the file. |
| D-48 | `doctor::Finding` carries one `fix: String`, but a broken log trail has two or three ranked remedies; and `RunCheck = fn(&Snapshot)` has no repo root, so a fix naming a file prints an absolute path. | **Both real, both DEFERRED to v0.2, neither blocking.** `Finding.fix: Fixes` is the right shape (it is what `KsError` already carries) but `Finding` is a JSON contract agents branch on, so it is a v0.2 cut, not an integration rider. The absolute path is cosmetic and pre-existing (`check_reserved_keys` already had it); the clean fix is to post-process `Finding.fix` in `cmd/doctor.rs`, where a `Ctx` is in hand — deliberately NOT by giving checks a root, which would weaken the `fn(&Snapshot)` purity `purity.rs` enforces. `cmd/repair.rs`'s gate fix line has the same cosmetic issue for the same reason (`plan_repair` is pure). |
| D-49 | `Staleness::NeverScanned` now has two causes — no anchor at all (wiped/never scanned) vs. a spec never committed to the main ref — and `stale_fix` names `kanspec scan` for both, which is a no-op for the second. | **Accepted for v0.1; the honest answer is still `never scanned`.** Distinguishing them needs a field on the variant or a new one, both forbidden this round. Consequence to know on rollout: a spec created under `sync = "batch"` shows in `features --stale` until its tracker commit lands. `status` stays quiet either way (`attention` ignores both `Ok` and `NeverScanned`). |
| D-50 | `out::visible_len` counts `char`s, so a CJK or emoji grapheme measures 1 where a terminal draws 2, and it ends an escape at the first `m` (right for SGR, wrong for OSC-8). | **First half CLOSED (round F); second half still open, deliberately.** The width bug was reachable after all — not through ids (ASCII by construction) but through TITLES, which are free text: a 16-character CJK title measured 16 where the terminal drew 32, so `Line::write` right-aligned the `→ fix` into columns that did not exist and the line WRAPPED, destroying the alignment `out.rs` exists to produce. `unicode-width` was authorised and taken, at ZERO supply-chain cost — `comfy-table`, which measures the board's columns, already pulls that exact crate, so the crate's two width models now agree by construction rather than by luck. Measured over the RUN rather than per `char`, so a ZWJ emoji sequence and a combining mark count once; ASCII is byte-for-byte unchanged, and every kanspec glyph (`○ ◐ ◈ ⇂ ● ✕ ✗ ✓ → ◇ · ⧗ ⚠`) is Ambiguous or Neutral and still measures 1, so no existing alignment moved. `Line::write`'s tail carried the same undercount spelled `t.chars().count()` and is now columns too. The OSC-8 half stays open: an escape still ends at the first `m`, which is right for every escape kanspec emits and wrong for hyperlinks it does not emit. |
| D-51 | Two agents wrote outside their assigned file lists: the `repair` fix touched `derive.rs` and `tests/doctor_replay.rs`; the projections fix touched `instructions.rs` and `cmd/done.rs`. `derive.rs` was edited by **two** branches in the same wave. | **All four accepted; no conflict occurred.** The two `derive.rs` edits are textually disjoint (the attestation seam near the top and `badge`; `staleness` and the test module ~300 lines down) and semantically independent — different functions, no shared state — so `git merge` auto-resolved and both are verified live. Each excursion was forced and correctly reported rather than hidden: `derive::badge` is the single seam `ls`/`show`/`board`/`flow` all read, so editing it beat editing four call sites; the agent snippet const cannot move to `setup.rs` without breaking `single_write_path.rs`; and `done.rs`'s one added `regenerate` line was demanded by a new failing guard test. **The lesson for the next wave is that ownership lists should be derived from the fix, not assigned ahead of it** — three of five findings could not be fixed inside their nominal list. |
| D-52 | Three existing tests asserted the defects being fixed and had to be changed. | **All three changes upheld after reading them; none was a weakening, and none was deleted** (verified by diffing the full test-name set before and after: 465 → 508, zero removals). `doctor_replay.rs`'s `an_attested_repair_line_makes_a_diverged_ticket_replay_clean_again` asserted that an unmerged ticket attested into `done` produced *zero* findings — i.e. it asserted the laundering route was fine; its fixture moved to a non-terminal attestation, preserving its stated purpose, and the terminal case it used to bless is now covered by a new test that asserts the opposite. `a_hand_edited_state_is_caught_by_the_very_next_run` asserted the fix line *should* be `kanspec repair …`, the prescription being removed; it was inverted. `cmd/repair.rs`'s `a_hand_edited_state_is_recoverable_by_attestation` built frontmatter `done` over a log stopping at `review` and asserted `plan` **succeeded** — the exact two-command route, blessed by a unit test; its fixture moved to a non-terminal state. **That a defect is asserted by three passing tests is the finding, not a footnote:** the suite encoded the bug, so the bug was invisible to it by construction. |
| D-53 | `src/out.rs` is "frozen after wave 0" and grew two more `pub fn` — `spoken` / `spoken_as` — on top of the two D-42 blessed. | **Granted, same shape as D-42.** Two pure, total functions: no new state, no new variant, no new match arm, no new CLI arg. `spoken_as` is `pub` for exactly the D-43 reason (`Verb::command_as`): it is the seam `tests/render_color.rs` pins BOTH binary names through without racing the set-once `OnceLock`. The alternative — editing 123 call sites — is the change this avoids, and it would have to be repeated for every fix added afterwards. |
| D-54 | `derive::close_evidence` mirrors the `in main <sha>` note grammar that `scan.rs` writes, because `scan::CONFIRM_NOTE` and `scan::confirm_note_sha` are private. | **Duplication accepted for v0.1, because it is TESTED duplication.** The mirror is pinned by a test that runs the REAL `scan::plan_confirm` and reads its emitted note back through `derive::note_sha`, so a gate that changes its spelling fails loudly in `derive.rs` rather than silently ceasing to corroborate. Making the two items `pub(crate)` would delete the duplication outright and is the right v0.2 move; it was not taken here only because `scan.rs` belongs to another owner and the test makes the seam safe meanwhile. |
| D-55 | `doctor`'s registry grew a 14th check, `unproven_close`, at `Severity::Error` — so a repo that adopted kanspec before this and closed tickets by hand will fail CI on first run. | **Upheld; the noise is the finding, not a side effect.** A `done` ticket whose only evidence of a close is the close line itself is exactly the forgery invariant 10 could not see, and grading it a Warning would restore the silence being fixed. Three honest remedies are named, and `kanspec scan --explain <id>` clears it outright wherever the work really did land. The check stands down where it has nothing to ask git about (no `branch:`/`head:`/`pr:`), which is a real floor now written into DESIGN.md rather than papered over. |
| D-56 | `require`'s second fix was `allowed_slice(from)[0]`, which for a TERMINAL state names `Confirm` — a verb that bounces straight back. The advice chain never terminated. | **Fixed in `transitions::onward`.** A closed ticket is told the true final thing: look at what happened (`show`), or open a NEW ticket for the work that still wants doing. Both exit 0 on the spot, so the chain ends in a SUCCESS rather than merely giving up. Non-terminal refusals are byte-for-byte unchanged. The general lesson is recorded in D-57: runnability is a property of one arrow, termination is a property of the chain, and only the chain test finds a ring. |
| D-57 | Two more non-terminating advice chains were found at integration, both outside the four fix agents' file lists. | **Both fixed in round F.** (1) `src/triage.rs` — the `done` gates each suggested the flag that answers THEM and dropped every answer already given, so `--no-followups` → `--no-quirks` → `--no-followups` rang for ever on the ORDINARY close-out of a genuinely merged ticket (verified against the real binary). Every suggestion is now built as *what you already said* + *this gate's answer* via a private `carried(a, adds)`. `adds` is load-bearing: `--spawn`/`--no-followups`, `--quirk`/`--no-quirks` and `--decision`/`--no-decisions` are declared `conflicts_with` in `cli.rs`, so carrying the negative into a suggestion that supplies the positive emits a command clap REFUSES — a worse failure than the ring, because it does not even parse. (2) `src/git.rs::worktree_add` offered three fixes the CLI rejects: `kanspec where {branch}` (`where` takes `--branch`) and `kanspec start --no-worktree` twice (no such flag; a worktree is opt-in via `--worktree`, so plain `start <id>` IS the no-worktree claim). All three sat a dozen lines from a `cmd/flow.rs` refusal that spells the same advice correctly. |
| D-58 | Nothing parsed the fix strings raised OUTSIDE the transition layer, which is how the three `src/git.rs` commands in D-57 rotted unnoticed. | **Closed by a crate-wide guard**, `tests/cli_well_formed.rs::every_fix_that_names_kanspec_is_a_command_the_real_cli_accepts` (owner F). It extracts every `fix!("…")` literal under `src/` (skipping comment lines, so the doc comments that quote `fix!("kanspec …")` as prose are not mistaken for commands), substitutes each `{…}`/`<…>` hole by the flag it follows, and parses the result through the real `Cli::try_parse_from`. It fails RED on all three D-57 commands. Its declared boundary: it checks the SHAPE clap sees, not that the values are realistic, and it says nothing about whether following the arrows TERMINATES — that is the chain property D-56 covers. |

| D-59 | On the first migrated corpus (83 specs, 666 rules) the path-scoped spec section had no ceiling: the median commit's touched files injected ~1.1k tokens, a quarter over 2.5k, the widest 36-file commit ~14.6k — into every SessionStart and PreCompact. The "~1.5k" in DESIGN.md was written before any corpus existed. | **A ranked, soft, named budget — in the generator.** `[prime] spec_budget_tokens` (default 2 000; `0` lifts it). Matched specs rank by `Scope::rank` — most specific glob first (a spec that names the file is more that file's spec than one that owns the directory), then coverage of the touched paths, then name — and are shown in that order while the budget is unspent, so the first spec always shows whole and the payload overshoots by at most one spec; strict order rather than first-fit, because skipping a big spec to fit a small one behind it would put a lesser match in front of the agent and hide the file's own spec. Everything past the budget is NAMED with its rule count and `kanspec spec show <name>`. Spent is measured in the bytes the renderer emits, via the same line builders. The budget lives in `rulesdoc::build` so `rules --path` and `prime` elide identically (invariant 3, now proved under a spent budget too); `rules --full` is the human's way past it and `prime` has no such flag by construction (`build_full` is a separate entry point). Measured after: the widest branches inject ~2.5–3.3k tokens; a single-file scope shows the file's three exact-match specs and names the directory-level one. |
| D-60 | `doctor`'s `dead_globs` covered spec `code:` only; quirk `paths:` and decision `scope:` rotted unseen, and a quirk whose every path rotted is a landmine that silently stopped warning. | **Closed on the `scan` side, by one definition.** `GitState` gained `decision_dead_globs` / `quirk_dead_globs` (rotted entries only, standing records only — a revoked decision's scope is nobody's finding), computed by the same `dead_globs` helper the spec anchor uses, so the three kinds cannot rot by different tests. `doctor` grades them as it grades specs: some globs dead is a Warning, all dead is an Error — a quirk then warns nobody, a decision's body is injected for no path. Proved on the real binary in `tests/glob_rot.rs`, including the round trip: restore the path, rescan, the finding leaves. |

### Known limits carried forward, stated out loud

**R-1 · Multi-file plans are not atomic.** Per-file tmp+rename is; a plan touching a ticket *and* a
proposal *and* `comments.jsonl` can be interrupted between renames. Mitigated by preferring
single-file plans (a transition's frontmatter delta and its Log line are one write to one file),
ordering the authoritative file last, and a `doctor` check for half-applied plans. A real gap, not
a solved problem — DESIGN.md's own framing: gates on plain files are provable, not preventable.

**R-2 · The seals stop at the file boundary.** `sed -i 's/state: review/state: done/'` still works.
The answer is detection, not prevention: `replay` proves the state was never legally reached, at
the **next verb** and in CI. Module docs must say plainly that the seals bind the tool and the Log
binds the human.

**R-3 · Three invariants rest on grep tests** (one write path, derive purity, sealed proof) because
Rust cannot express "no `std::fs` in this crate". A `use std::fs as f;` alias walks past them. They
raise the cost of a violation from zero to "you had to work around a test named after the
invariant" — a deterrent, not a guarantee.

**R-4 · `unknown` will fire more than the user wants.** A multi-commit squash with a title-only
merge message and no `gh` defeats all four rungs; the fix (`gh auth login`) is outside kanspec's
control.

**R-5 · `serde_yaml_ng`'s last release is 2024-05-26.** Right choice today (positioned type errors,
2× faster, libyaml-stable, 5.6M recent downloads); exposure is one `from_str` call site;
`serde_yaml_bw` 2.5.7 is a one-import migration at the cost of line/column in type errors and 2×
parse time.

**R-6 · `Snapshot::load` is linear and unmemoized — including inside the server** (corrected in
round D; it previously read "outside the server", which assumed D-22's memo that D-40 declined to
build). 55ms at 2000 entities is fine; `transact` reloads it inside the lock on every write, and
`up` reloads it per request along with a fresh `Ctx`. The `rev` field exists from day one so the
escape hatch (mtime-keyed partial reload, or the sanctioned gitignored SQLite cache) is a one-file
change rather than a re-architecture. Nobody should build it before profiling says so — and for the
server specifically, the memo cannot be added without first unfreezing `Ctx::now` (D-40).

**R-7 · 4 hex is 65,536 ids, and the local exclusion guarantee evaporates across machines.**
Birthday risk near a few hundred ids created off-network. `doctor` detects duplicates after a
merge and `id_width` can grow, but the recovery — two entities that both think they are `t-9c41`,
already referenced by `deps:` elsewhere — is unpleasant and has no automated fix.

**R-8 · Cross-machine claims remain eventually consistent.** The lock is per-machine because the
workspace is per-machine. `scan`/`status` flag double-claims after sync; true atomicity needs a
shared service, deliberately out of scope.

**R-9 · `fm::set` on a multi-line value is a hard error**, correct today (no verb writes a block
scalar or nested map) but one schema field from making a verb unimplementable — and there is no
mature format-preserving YAML editor in Rust to fall back on.
