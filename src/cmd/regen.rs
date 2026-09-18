//! `regenerate [--stage] [--quiet]` and `merge-driver <base> <ours> <theirs> [path]` — the
//! two hidden verbs behind p-97d6 c5: **the generated projections never conflict, and a
//! merge commit carries a fresh regeneration of them.**
//!
//! # Why a merge driver cannot regenerate
//!
//! `KANSPEC-FEATURES.md` and `KANSPEC-ARCHITECTURE.md` are pure functions of the committed
//! corpus, so the right resolution of a conflict in either is "regenerate from the merged
//! store". A git merge driver cannot do that: since merge-ort (git 2.34) drivers run while
//! the tree merge is still in memory, before the working tree or the index sees a single
//! merged file. The three temp files the driver receives are all it can know.
//!
//! So the work is split in two, and both halves are installed by `init` beside the hooks:
//!
//! 1. **The driver** ([`merge_driver`]) makes the conflict go away. It tries a plain
//!    three-way text merge — two branches that each closed a different ticket usually
//!    merge cleanly at the line level — and when that conflicts it keeps *ours* and still
//!    exits 0, because the bytes it leaves are provisional either way.
//! 2. **The `post-merge` hook** runs [`regenerate`]` --stage --amend-merge` against the
//!    now-merged tree and folds the result into the merge commit git just made (in
//!    plumbing — see [`fold_into_head`]); the
//!    `pre-commit` hook, while `MERGE_HEAD` exists, does the same staging for the commit a
//!    human makes after resolving a conflict elsewhere (`git commit` re-reads the index
//!    after that hook, so no amend is needed there). Either way the commit that lands
//!    carries the two projections rendered from the merged store: no human resolves a
//!    table, and no `kanspec: regenerate` commit trails the merge.
//!
//! `pre-merge-commit` is the hook that LOOKS right and is not: `git merge` writes the
//! result tree before running it, so what it stages is stranded in the index beside a
//! merge commit that does not carry it. Hence the amend, and hence its guards
//! ([`fresh_local_merge_commit`]): only a merge commit, only the one whose first parent
//! is `ORIG_HEAD`, and only while no remote ref contains it.
//!
//! The driver alone would commit a stale projection; the hook alone would still leave a
//! conflict for a human. Together the invariant `project.rs` states — two clones of one
//! commit regenerate identical bytes — survives a merge.
//!
//! Neither verb transitions a ticket, so both go through `Store::transact` with
//! `verb = None`, and neither writes anything git would auto-commit under
//! `sync = "commit"` (`Op::WriteGenerated` is excluded from that pathspec by design).

use std::path::PathBuf;

use serde::Serialize;

use crate::cli::{MergeDriverArgs, RegenerateArgs};
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::out::{glyph, Line, Render, Style};
use crate::plan::Plan;
use crate::store::Store;
use crate::{fix, fixes};

#[derive(Debug, Serialize)]
pub struct RegenerateReport {
    /// the projections whose bytes moved
    pub regenerated: Vec<PathBuf>,
    /// the projections `--stage` handed to `git add`
    pub staged: Vec<PathBuf>,
    /// `--amend-merge` folded a changed projection into the merge commit git just made
    pub amended: bool,
    /// why `--amend-merge` left the commit alone, when it did — the guard that refused
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amend_skipped: Option<String>,
    #[serde(skip)]
    quiet: bool,
}

/// Render both projections from the store as it is on disk right now, and with `--stage`
/// put whichever exist into the index. The hooks call this with `--quiet --stage`; a human
/// calls it bare after a merge that left the tree dirty.
pub fn regenerate(ctx: &Ctx, a: &RegenerateArgs) -> Result<RegenerateReport> {
    ctx.require_initialized()?;
    // The plan itself is the projection writes; `transact` re-renders from the
    // post-write snapshot (they are identical here — nothing else is written) and skips
    // any file whose bytes already match, so an unchanged corpus stays `git status`-clean.
    let committed = Store::open(ctx).transact(None, &ctx.invocation(), |s, _m| {
        Ok(Plan::of(crate::project::plan_regenerate(s, &ctx.layout)))
    })?;
    let root = ctx.layout.repo_root().to_path_buf();
    let mut regenerated: Vec<PathBuf> = committed
        .touched
        .iter()
        .map(|p| crate::hooks::relative_to(&root, p))
        .collect();
    regenerated.sort();
    regenerated.dedup();

    let mut staged = Vec::new();
    if a.stage {
        for p in [ctx.layout.features_md(), ctx.layout.architecture_md()] {
            if !p.exists() {
                continue;
            }
            let rel = crate::hooks::relative_to(&root, p);
            let o = ctx.git.run(&["add", "--", &rel.to_string_lossy()])?;
            if o.code == 0 {
                staged.push(rel);
            }
        }
    }
    let mut amended = false;
    let mut amend_skipped = None;
    if a.amend_merge && !staged.is_empty() {
        let specs: Vec<String> = staged
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let mut args = vec!["diff", "--cached", "--quiet", "--"];
        args.extend(specs.iter().map(String::as_str));
        // `diff --cached --quiet` exits 1 when the index differs from HEAD on those paths —
        // i.e. when the regeneration actually changed what the merge commit carries.
        if ctx.git.run(&args)?.code != 1 {
            amend_skipped = Some("the merge commit already carries this regeneration".into());
        } else if let Err(why) = fresh_local_merge_commit(ctx) {
            amend_skipped = Some(why);
        } else {
            match fold_into_head(ctx) {
                Ok(()) => amended = true,
                Err(why) => amend_skipped = Some(why),
            }
        }
    }
    Ok(RegenerateReport {
        regenerated,
        staged,
        amended,
        amend_skipped,
        quiet: a.quiet,
    })
}

/// Re-make HEAD from the current index with HEAD's own parents and message — what
/// `git commit --amend --no-edit` would do, spelled in plumbing because porcelain refuses
/// inside the `post-merge` hook (`MERGE_HEAD` is still on disk while it runs: "You are in
/// the middle of a merge -- cannot amend"). `update-ref` is given the old value, so a HEAD
/// that moved underneath us is a refusal rather than a clobber, and the reflog says who
/// rewrote it and why.
fn fold_into_head(ctx: &Ctx) -> std::result::Result<(), String> {
    let run = |args: &[&str]| -> std::result::Result<String, String> {
        let o = ctx.git.run(args).map_err(|e| e.to_string())?;
        if o.code != 0 {
            return Err(format!("git {} failed: {}", args.join(" "), o.err.trim()));
        }
        Ok(o.out)
    };
    let old = run(&["rev-parse", "--verify", "HEAD^{commit}"])?
        .trim()
        .to_string();
    let tree = run(&["write-tree"])?.trim().to_string();
    let message = run(&["log", "-1", "--format=%B", "HEAD"])?;
    let parents: Vec<String> = run(&["rev-parse", "HEAD^@"])?
        .lines()
        .map(str::to_string)
        .collect();
    let mut args = vec!["commit-tree", tree.as_str()];
    for p in &parents {
        args.push("-p");
        args.push(p);
    }
    let message = message.trim_end().to_string();
    args.push("-m");
    args.push(&message);
    let new = run(&args)?.trim().to_string();
    run(&[
        "update-ref",
        "-m",
        "kanspec: fold the regenerated projections into the merge commit",
        "HEAD",
        &new,
        &old,
    ])?;
    Ok(())
}

/// The three guards behind `--amend-merge`, every one asked of git: HEAD has a second
/// parent (it is a merge commit), its first parent is `ORIG_HEAD` (it is the merge that
/// just moved us, not one we fast-forwarded onto), and no remote-tracking ref contains it
/// (nobody else has it yet). A fast-forward, a squash (`--squash` commits nothing, so HEAD
/// is still ORIG_HEAD) and a pulled-in merge commit all fail one of them and are left
/// alone. `Err` carries the guard that refused, for the report.
fn fresh_local_merge_commit(ctx: &Ctx) -> std::result::Result<(), String> {
    let sha = |rev: &str| {
        ctx.git
            .head_sha(rev)
            .ok()
            .map(|h| h.sha().as_str().to_string())
    };
    let (Some(head), Some(first), Some(orig)) = (sha("HEAD"), sha("HEAD^1"), sha("ORIG_HEAD"))
    else {
        return Err("HEAD, HEAD^1 or ORIG_HEAD does not resolve".into());
    };
    if sha("HEAD^2").is_none() {
        return Err("HEAD is not a merge commit".into());
    }
    if head == orig || first != orig {
        return Err("HEAD is not the merge commit that just moved ORIG_HEAD".into());
    }
    let o = ctx
        .git
        .run(&["branch", "-r", "--contains", "HEAD"])
        .map_err(|e| e.to_string())?;
    if o.code != 0 {
        return Err(format!("git branch -r --contains failed: {}", o.err.trim()));
    }
    if !o.out.trim().is_empty() {
        return Err(format!(
            "a remote ref already contains HEAD: {}",
            o.out.trim()
        ));
    }
    Ok(())
}

impl Render for RegenerateReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        if self.regenerated.is_empty() {
            writeln!(w, " · projections already current")?;
        }
        for p in &self.regenerated {
            Line::new(glyph::OK, format!("regenerated {}", p.display())).write(w, st)?;
        }
        for p in &self.staged {
            Line::new(glyph::OK, format!("staged {}", p.display())).write(w, st)?;
        }
        if self.amended {
            Line::new(glyph::OK, "folded into the merge commit".to_string()).write(w, st)?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct MergeDriverReport {
    /// `true` when the plain three-way merge was clean and its result was written
    pub merged_clean: bool,
}

/// `merge.kanspec.driver`. Never a conflict, never chatty: git ignores the driver's stdout
/// and treats any non-zero exit as a conflict, and a conflict in a generated file is
/// exactly what this verb exists to retire. See the module header for why it cannot
/// regenerate here and what does instead.
pub fn merge_driver(ctx: &Ctx, a: &MergeDriverArgs) -> Result<MergeDriverReport> {
    // `merge-file` rewrites `ours` IN PLACE with the three-way result and exits with the
    // number of conflicts; a negative exit is an error. On a conflict it has already
    // written conflict markers into `ours`, so `--ours` re-resolves every hunk to our side:
    // the bytes are provisional either way (the hook regenerates them), and a generated
    // file must never reach a human wearing markers. Git, not this crate, moves the bytes —
    // `tests/single_write_path.rs` holds every file but `store.rs` to that.
    let ours = a.ours.to_string_lossy().into_owned();
    let base = a.base.to_string_lossy().into_owned();
    let theirs = a.theirs.to_string_lossy().into_owned();
    let o = ctx
        .git
        .run(&["merge-file", "--ours", &ours, &base, &theirs])?;
    if o.code < 0 {
        return Err(KsError::invalid(
            format!("git merge-file failed on {ours}: {}", o.err.trim()),
            fixes![fix!("kanspec regenerate")],
        ));
    }
    Ok(MergeDriverReport {
        merged_clean: o.code == 0,
    })
}

impl Render for MergeDriverReport {
    /// Silent by contract: git is the only caller.
    fn human(&self, _w: &mut dyn std::io::Write, _st: &Style) -> std::io::Result<()> {
        Ok(())
    }
}
